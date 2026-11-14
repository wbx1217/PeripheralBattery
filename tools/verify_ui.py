"""一键跑一遍界面验收：启动程序 → 截亮色面板 → 切暗色 → 截暗色 → 右键托盘量菜单位置。

为什么非得一口气跑完：DSH 的 PowerShell 沙箱在命令结束时会把子进程一起收掉，
分两次调用的话第二次面对的已经是个死进程。

托盘菜单不是靠鼠标事件触发的 —— 程序只认 `WM_TRAY`（Shell_NotifyIcon 的回调）
里的 `LOWORD(lParam)` 事件码，所以这里直接 PostMessage 一个 `0x0205`
（`WM_RBUTTONUP`），锚点由程序自己读光标位置。

用法：
    python tools/verify_ui.py            # 图存到 app/assets/，打印菜单与图标的几何关系
"""

import ctypes
import subprocess
import sys
import time
from ctypes import wintypes

here = __file__.rsplit("\\", 1)[0]
sys.path.insert(0, here)
import shot  # noqa: E402
import shot_window  # noqa: E402
import trayctl  # noqa: E402

user32 = shot.user32
for name, argument_types, result_type in [
    ("FindWindowW", [wintypes.LPCWSTR, wintypes.LPCWSTR], wintypes.HWND),
    ("EnumWindows", [ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM), wintypes.LPARAM], wintypes.BOOL),
    ("GetClassNameW", [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int], ctypes.c_int),
    ("GetWindowThreadProcessId", [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)], wintypes.DWORD),
    ("PostMessageW", [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM], wintypes.BOOL),
    ("GetWindowRect", [wintypes.HWND, ctypes.POINTER(wintypes.RECT)], wintypes.BOOL),
    ("ShowWindow", [wintypes.HWND, ctypes.c_int], wintypes.BOOL),
    ("SetForegroundWindow", [wintypes.HWND], wintypes.BOOL),
]:
    function = getattr(user32, name)
    function.argtypes = argument_types
    function.restype = result_type

EXE = r"target\release\外设电量显示.exe"
TITLE = "外设电量显示"
WM_TRAY = 0x8001
WM_RBUTTONUP = 0x0205
# 页眉三个按钮（28px 热区）在 150% 缩放下的物理中心点。
THEME_BUTTON = (475, 55)


def save_window(path: str) -> None:
    hwnd = user32.FindWindowW(None, TITLE)
    if not hwnd:
        raise SystemExit("找不到面板窗口")
    width, height, raw = shot_window.grab(hwnd)
    stride = (width * 3 + 3) & ~3
    rows = [
        raw[(height - 1 - row) * stride : (height - 1 - row) * stride + width * 3]
        for row in range(height)
    ]
    rgb = [
        b"".join(bytes((line[i + 2], line[i + 1], line[i])) for i in range(0, len(line), 3))
        for line in rows
    ]
    shot.write_png(path, width, height, rgb)
    print(f"saved {path} {width}x{height}")


def click_client(hwnd: int, x: int, y: int) -> None:
    lp = (y << 16) | x
    user32.PostMessageW(hwnd, 0x0200, 0, lp)
    user32.PostMessageW(hwnd, 0x0201, 1, lp)
    user32.PostMessageW(hwnd, 0x0202, 0, lp)


def windows_of_class(class_name: str) -> list[tuple[int, int, int, int, int]]:
    """返回 (hwnd, left, top, width, height) 列表。"""
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def callback(hwnd, _):
        name = ctypes.create_unicode_buffer(64)
        user32.GetClassNameW(hwnd, name, 64)
        if name.value == class_name:
            rect = wintypes.RECT()
            user32.GetWindowRect(hwnd, ctypes.byref(rect))
            found.append((hwnd, rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top))
        return True

    user32.EnumWindows(callback, 0)
    return found


def our_tray_window(pid: int) -> int | None:
    for hwnd, left, top, width, height in windows_of_class("PeripheralBatteryTrayWnd"):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid:
            return hwnd
    return None


def main() -> None:
    user32.SetProcessDPIAware()
    process = subprocess.Popen([EXE])
    try:
        time.sleep(6)
        hwnd = user32.FindWindowW(None, TITLE)
        if not hwnd:
            raise SystemExit("程序没起来，或者窗口标题变了")
        user32.ShowWindow(hwnd, 5)
        user32.SetForegroundWindow(hwnd)
        user32.SetCursorPos(60, 60)  # 光标移开，免得按钮上带 hover 底色
        time.sleep(0.6)
        save_window("app/assets/_panel_light.png")

        click_client(hwnd, *THEME_BUTTON)
        time.sleep(0.5)
        user32.SetCursorPos(60, 60)
        user32.PostMessageW(hwnd, 0x0200, 0, (5 << 16) | 5)  # 让 GPUI 清掉按钮 hover
        time.sleep(0.5)
        save_window("app/assets/_panel_dark.png")

        # 切回亮色，免得把用户的偏好留在暗色。
        click_client(hwnd, *THEME_BUTTON)
        time.sleep(0.6)

        tray = our_tray_window(process.pid)
        if not tray:
            raise SystemExit("找不到本次进程的托盘窗口")
        icon = trayctl.icon_rect()
        cx, cy = (icon[0] + icon[2]) // 2, (icon[1] + icon[3]) // 2
        print(f"tray icon={icon} center=({cx}, {cy})")
        user32.SetCursorPos(cx, cy)
        time.sleep(0.2)
        user32.PostMessageW(tray, WM_TRAY, 0, WM_RBUTTONUP)
        time.sleep(0.8)

        menus = windows_of_class("#32768")
        print(f"menu rects={[(m[1], m[2], m[3], m[4]) for m in menus]}")
        if menus:
            _, left, top, width, height = menus[0]
            print(
                f"menu {width}x{height} at ({left}, {top}); icon center ({cx}, {cy}); "
                f"dx={left - cx}, dy={top - cy}, gap_to_icon_top={cy - (top + height)}"
            )
        shot.capture("app/assets/_tray_menu.png", (max(0, cx - 330), max(0, cy - 320), 660, 400))

        for menu, *_ in menus:
            user32.PostMessageW(menu, 0x0100, 0x1B, 0)  # WM_KEYDOWN VK_ESCAPE
            user32.PostMessageW(menu, 0x0101, 0x1B, 0)
        time.sleep(0.4)
        print(f"menu after cancel={len(windows_of_class('#32768'))}")
    finally:
        process.terminate()
        print("app terminated")


if __name__ == "__main__":
    main()
