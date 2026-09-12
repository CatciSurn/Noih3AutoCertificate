use crate::Result;
use std::path::Path;
use std::process::ExitStatus;

/// The bundled v0.5 signer emits CP936 (GBK), even in a UTF-8 console.
/// Transcode its output rather than changing the console's shared code page.
#[cfg(windows)]
pub fn run(exe: &Path, tool_dir: &Path) -> Result<ExitStatus> {
    let mut command = std::process::Command::new(exe);
    command.current_dir(tool_dir);
    windows::run_command(&mut command).map_err(|error| {
        format!(
            "运行改签工具或读取其输出失败 {}（工作目录 {}）：{error}",
            exe.display(),
            tool_dir.display()
        )
    })
}

#[cfg(not(windows))]
pub fn run(_exe: &Path, _tool_dir: &Path) -> Result<ExitStatus> {
    Err("运行改签工具仅支持 Windows。".into())
}

#[cfg(windows)]
mod windows {
    use std::io::{self, Read, Write};
    use std::process::{Command, ExitStatus, Stdio};

    #[link(name = "kernel32")]
    extern "system" {
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            input: *const u8,
            input_len: i32,
            output: *mut u16,
            output_len: i32,
        ) -> i32;
    }

    pub(super) fn run_command(command: &mut Command) -> io::Result<ExitStatus> {
        let mut child = command
            // Keep keyboard access for the signer's prompts and `pause`.
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped signer stdout");
        let stderr = child.stderr.take().expect("piped signer stderr");
        std::thread::scope(|scope| {
            // Drain both pipes concurrently, including prompts without a newline.
            let stdout_thread = scope.spawn(|| forward_and_drain(stdout, io::stdout()));
            let stderr_thread = scope.spawn(|| forward_and_drain(stderr, io::stderr()));
            let status = child.wait();
            let stdout_result = stdout_thread
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("读取改签工具标准输出的线程异常")));
            let stderr_result = stderr_thread
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("读取改签工具错误输出的线程异常")));
            let status = status?;
            stdout_result?;
            stderr_result?;
            Ok(status)
        })
    }

    fn forward_and_drain(mut input: impl Read, output: impl Write) -> io::Result<()> {
        let result = forward_gbk(&mut input, output);
        if result.is_err() {
            // A display/decoding failure must not leave the child blocked on a
            // full pipe. The caller reports failure and prevents save writeback.
            let _ = io::copy(&mut input, &mut io::sink());
        }
        result
    }

    fn forward_gbk(mut input: impl Read, mut output: impl Write) -> io::Result<()> {
        let mut decoder = GbkDecoder::default();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = match input.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                return decoder.finish();
            }
            let text = decoder.push(&buffer[..count])?;
            output.write_all(text.as_bytes())?;
            output.flush()?;
        }
    }

    #[derive(Default)]
    struct GbkDecoder {
        pending: Vec<u8>,
    }

    impl GbkDecoder {
        fn push(&mut self, bytes: &[u8]) -> io::Result<String> {
            self.pending.extend_from_slice(bytes);
            let mut end = 0;
            while end < self.pending.len() {
                if (0x81..=0xfe).contains(&self.pending[end]) {
                    if end + 1 == self.pending.len() {
                        break;
                    }
                    end += 2;
                } else {
                    end += 1;
                }
            }
            let text = decode_gbk(&self.pending[..end])?;
            // Preserve a lead byte if a pipe read splits a Chinese character.
            self.pending.drain(..end);
            Ok(text)
        }

        fn finish(&self) -> io::Result<()> {
            if self.pending.is_empty() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "改签工具输出以不完整的 GBK 字符结束，无法确认完整结果",
                ))
            }
        }
    }

    fn decode_gbk(bytes: &[u8]) -> io::Result<String> {
        if bytes.is_empty() {
            return Ok(String::new());
        }
        let len = i32::try_from(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "改签工具输出过长"))?;
        let mut wide = vec![0_u16; bytes.len()];
        const CP_GBK: u32 = 936;
        const MB_ERR_INVALID_CHARS: u32 = 0x8;
        // CP936 produces at most one UTF-16 unit per input byte. Both buffers
        // remain valid for the call; malformed input is reported, never replaced.
        let count = unsafe {
            MultiByteToWideChar(
                CP_GBK,
                MB_ERR_INVALID_CHARS,
                bytes.as_ptr(),
                len,
                wide.as_mut_ptr(),
                len,
            )
        };
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "无法按 GBK 解码改签工具输出：{}",
                    io::Error::last_os_error()
                ),
            ));
        }
        String::from_utf16(&wide[..count as usize])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // Actual CP936 bytes of the bundled signer's success message.
        const SUCCESS: &[u8] = b"\xd2\xd1\xce\xaa\xc4\xe3\xb8\xc4\xd0\xb4\xc7\xa9\xc3\xfb";

        #[test]
        fn decodes_success_message_across_every_pipe_boundary() {
            for split in 0..=SUCCESS.len() {
                let mut decoder = GbkDecoder::default();
                let mut text = decoder.push(&SUCCESS[..split]).unwrap();
                text.push_str(&decoder.push(&SUCCESS[split..]).unwrap());
                decoder.finish().unwrap();
                assert_eq!(text, "已为你改写签名");
            }
            let mut decoder = GbkDecoder::default();
            let mut text = String::new();
            for byte in SUCCESS {
                text.push_str(&decoder.push(&[*byte]).unwrap());
            }
            decoder.finish().unwrap();
            assert_eq!(text, "已为你改写签名");
        }

        #[test]
        fn preserves_ascii_line_endings_and_gbk_ascii_range_trail_bytes() {
            let bytes = [b"SAVEDATA.BIN\r\n".as_slice(), b"\x81\x40", SUCCESS, b"!"].concat();
            let mut output = Vec::new();
            forward_gbk(bytes.as_slice(), &mut output).unwrap();
            assert_eq!(
                String::from_utf8(output).unwrap(),
                "SAVEDATA.BIN\r\n丂已为你改写签名!"
            );
        }

        #[test]
        fn reports_invalid_or_truncated_gbk_instead_of_garbled_success() {
            assert!(decode_gbk(b"\x81\x30").is_err());
            let mut output = Vec::new();
            assert!(forward_gbk(b"\xd2".as_slice(), &mut output).is_err());
        }

        #[test]
        fn flushes_a_prompt_before_reading_more_output() {
            use std::cell::Cell;
            let flushed = Cell::new(false);
            struct PromptReader<'a> {
                flushed: &'a Cell<bool>,
                sent: bool,
            }
            impl Read for PromptReader<'_> {
                fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                    if self.sent {
                        assert!(self.flushed.get(), "prompt must be visible before waiting");
                        return Ok(0);
                    }
                    self.sent = true;
                    let prompt = b"Press any key to continue . . .";
                    bytes[..prompt.len()].copy_from_slice(prompt);
                    Ok(prompt.len())
                }
            }
            struct PromptWriter<'a>(&'a Cell<bool>);
            impl Write for PromptWriter<'_> {
                fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                    assert_eq!(bytes, b"Press any key to continue . . .");
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> io::Result<()> {
                    self.0.set(true);
                    Ok(())
                }
            }
            forward_gbk(
                PromptReader {
                    flushed: &flushed,
                    sent: false,
                },
                PromptWriter(&flushed),
            )
            .unwrap();
        }
    }
}
