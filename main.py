import tkinter as tk
from tkinter import messagebox
from PIL import Image, ImageTk
import subprocess
import os
import sys
import re
import shutil
import time

# ================= 配置与路径处理 =================

def get_script_dir():
    """获取程序运行时的根目录（兼容脚本和打包后的EXE）"""
    if getattr(sys, 'frozen', False):
        return os.path.dirname(sys.executable)
    else:
        return os.path.dirname(os.path.abspath(__file__))

# 基础路径定义
ROOT_DIR = get_script_dir()
USER_PROFILE = os.environ['USERPROFILE']
# 游戏存档本地路径
NIOH3_LOCAL_BASE = os.path.join(USER_PROFILE, r"AppData\Local\KoeiTecmo\NIOH3\Savedata\76561198420813649")
# 改签工具相关路径
TOOL_DIR = os.path.join(ROOT_DIR, "Nioh3SaveCertificateTool")
OTHER_SAVE_IN = os.path.join(TOOL_DIR, "别人存档扔里面")
MY_SAVE_IN = os.path.join(TOOL_DIR, "你的存档扔里面")
TOOL_EXE = os.path.join(TOOL_DIR, "双击我改签v0.5.exe")

# ================= 目标 1: GUI 确认界面 =================

def show_image_and_confirm():
    """目标 1: 显示确认图片与提示信息"""
    root = tk.Tk()
    root.title("Nioh 3 存档管理助手")
    
    # 窗口居中
    window_width, window_height = 800, 700
    x = (root.winfo_screenwidth() - window_width) // 2
    y = (root.winfo_screenheight() - window_height) // 2
    root.geometry(f"{window_width}x{window_height}+{x}+{y}")
    
    main_frame = tk.Frame(root, padx=20, pady=20)
    main_frame.pack(fill=tk.BOTH, expand=True)
    
    tk.Label(main_frame, text="第一步：状态确认", font=("微软雅黑", 16, "bold")).pack(pady=5)
    
    msg = "请确认：\n1. 已新建存档\n2. 游戏主界面如下\n3. CONTINUE 和 LOAD GAME 可点击"
    tk.Label(main_frame, text=msg, font=("微软雅黑", 11), justify=tk.LEFT, bg="#f0f0f0", padx=10, pady=10).pack(fill=tk.X, pady=10)
    
    # 加载图片
    img_path = os.path.join(ROOT_DIR, "image.png")
    if os.path.exists(img_path):
        try:
            img = Image.open(img_path)
            img.thumbnail((700, 350), Image.Resampling.LANCZOS)
            photo = ImageTk.PhotoImage(img)
            lbl = tk.Label(main_frame, image=photo, bg="#ccc")
            lbl.image = photo
            lbl.pack(pady=5)
        except:
            tk.Label(main_frame, text="[图片加载失败]", fg="red").pack()
    
    confirmed = [False]
    def start(): confirmed[0] = True; root.destroy()
    def exit_pgm(): root.destroy()
    
    btn_frame = tk.Frame(main_frame)
    btn_frame.pack(pady=20)
    tk.Button(btn_frame, text="✓ 我已准备好", command=start, bg="#4CAF50", fg="white", font=("微软雅黑", 12), padx=30).pack(side=tk.LEFT, padx=10)
    tk.Button(btn_frame, text="✗ 取消操作", command=exit_pgm, bg="#f44336", fg="white", font=("微软雅黑", 12), padx=30).pack(side=tk.LEFT, padx=10)
    
    root.mainloop()
    return confirmed[0]

# ================= 目标 2: 禁用云存档逻辑 =================

def disable_steam_cloud(app_id="4198760"):
    """目标 2: 修改 sharedconfig.vdf 禁用云存档"""
    print(f"\n[2/4] 正在尝试关闭 AppID {app_id} 的云存档...")
    
    # 查找 Steam 路径
    steam_path = None
    for p in [r"C:\Program Files (x86)\Steam", r"D:\Steam", r"E:\Steam"]:
        if os.path.exists(p): steam_path = p; break
    
    if not steam_path:
        print("✗ 找不到 Steam 安装目录，请手动关闭云存档。")
        return False

    # 查找用户 ID (userdata)
    ud_path = os.path.join(steam_path, "userdata")
    if not os.path.exists(ud_path): return False
    u_dirs = [d for d in os.listdir(ud_path) if d.isdigit()]
    if not u_dirs: return False
    u_dirs.sort(key=lambda d: os.path.getmtime(os.path.join(ud_path, d)), reverse=True)
    user_id = u_dirs[0]

    # 修改文件
    vdf_path = os.path.join(steam_path, "userdata", user_id, "7", "remote", "sharedconfig.vdf")
    if not os.path.exists(vdf_path): return False

    # 强杀 Steam 进程防止覆盖
    subprocess.run("taskkill /F /IM steam.exe", shell=True, capture_output=True)
    time.sleep(1)

    try:
        with open(vdf_path, 'r', encoding='utf-8', errors='ignore') as f:
            content = f.read()
        
        # 修正后的正则：匹配指定 AppID 区块内的 CloudEnabled 项
        pattern = rf'("{app_id}"\s*\{{[^}}]*?)"CloudEnabled"\s*"\d"'
        if re.search(pattern, content, re.IGNORECASE | re.DOTALL):
            new_content = re.sub(pattern, r'\g<1>"CloudEnabled"		"0"', content, flags=re.IGNORECASE | re.DOTALL)
            with open(vdf_path, 'w', encoding='utf-8') as f:
                f.write(new_content)
            print(f"✓ 已成功在配置文件中禁用游戏 {app_id} 的云存档。")
            return True
        else:
            print("! 未在配置文件中找到该游戏的云同步项，可能已是关闭状态。")
            return True
    except Exception as e:
        print(f"✗ 修改 VDF 出错: {e}")
        return False

# ================= 目标 3 & 4: 存档处理逻辑 =================

def run_tool():
    """运行改签工具并等待用户操作完成"""
    if not os.path.exists(TOOL_EXE):
        print(f"✗ 找不到改签工具: {TOOL_EXE}")
        return False
    print(f"\n>>> 请在弹出的工具窗口点击改签按钮。完成后关闭工具窗口以继续...")
    proc = subprocess.Popen(f'"{TOOL_EXE}"', cwd=TOOL_DIR, shell=True)
    proc.wait() # 关键：等待用户手动关闭工具
    return True

def process_saves(save_type):
    """目标 4: 执行复杂的文件流转逻辑"""
    src_base = os.path.join(ROOT_DIR, "HandMakeSave" if save_type == "1" else "MagicMakeSave")
    
    if not os.path.exists(src_base):
        print(f"✗ 找不到源存档目录: {src_base}")
        return

    print(f"\n[4/4] 开始执行 {('手搓' if save_type=='1' else '魔改')} 存档流转...")

    try:
        # --- 子步骤 1: 处理 SYSTEMSAVEDATA00 ---
        print("\n正在处理系统设置存档 (SYSTEMSAVEDATA00)...")
        # 复制 别人(源) 到 扔里面
        shutil.copy2(os.path.join(src_base, "SYSTEMSAVEDATA00", "SAVEDATA.BIN"), os.path.join(OTHER_SAVE_IN, "SAVEDATA.BIN"))
        # 复制 你的(本地) 到 扔里面
        shutil.copy2(os.path.join(NIOH3_LOCAL_BASE, "SYSTEMSAVEDATA00", "SAVEDATA.BIN"), os.path.join(MY_SAVE_IN, "SAVEDATA.BIN"))
        
        # 运行改签
        run_tool()
        
        # 改签后写回本地
        shutil.copy2(os.path.join(OTHER_SAVE_IN, "SAVEDATA.BIN"), os.path.join(NIOH3_LOCAL_BASE, "SYSTEMSAVEDATA00", "SAVEDATA.BIN"))
        print("✓ 系统设置存档处理完成。")

        # --- 子步骤 2: 处理 SAVEDATA00 ---
        print("\n正在处理角色数据存档 (SAVEDATA00)...")
        # 复制 别人(源) 到 扔里面 (注意此处路径是 src_base/SAVEDATA00)
        shutil.copy2(os.path.join(src_base, "SAVEDATA00", "SAVEDATA.BIN"), os.path.join(OTHER_SAVE_IN, "SAVEDATA.BIN"))
        
        # 再次运行改签
        run_tool()
        
        # 改签后写回本地
        target_dir = os.path.join(NIOH3_LOCAL_BASE, "SAVEDATA00")
        if not os.path.exists(target_dir): os.makedirs(target_dir)
        shutil.copy2(os.path.join(OTHER_SAVE_IN, "SAVEDATA.BIN"), os.path.join(target_dir, "SAVEDATA.BIN"))
        print("✓ 角色数据存档处理完成。")

        return True
    except Exception as e:
        print(f"✗ 存档流转过程中出错: {e}")
        return False

# ================= 主程序入口 =================

def main():
    print("="*60)
    print("Nioh 3 存档自动化改签工具 v0.6")
    print("="*60)

    # 1. 状态确认
    if not show_image_and_confirm():
        print("用户取消操作。")
        return

    # 2. 禁用云存档 (AppID: 4198760)
    disable_steam_cloud("4198760")

    # 3. 询问存档类型
    print("\n[3/4] 存档类型选择：")
    print("1. 手搓存档 (【作者推荐】手工打造，更稳定)")
    print("2. 魔改存档 (各种满属性，可能影响平衡)")
    choice = input("\n请选择 (1 或 2): ").strip()
    if choice not in ["1", "2"]:
        print("无效输入，默认选择 1 (手搓存档)。")
        choice = "1"

    # 4. 执行改签流转
    if process_saves(choice):
        print("\n" + "★"*20)
        print("全部流程执行完毕！")
        print("请现在启动 Steam 并进入游戏。")
        print("★"*20)
    
    input("\n按回车键退出程序...")

if __name__ == "__main__":
    main()