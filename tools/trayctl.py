"""通过 Win32 找到本程序的托盘图标，并模拟左键/右键点击。

用法：
    python tools/trayctl.py rect          # 打印图标矩形
    python tools/trayctl.py left          # 左键单击（切换面板）
    python tools/trayctl.py right         # 右键单击（弹出菜单）

消息窗是 HWND_MESSAGE 的，`EnumWindows` 看不到，所以按类名用
`FindWindowExW(HWND_MESSAGE, ...)` 找；图标矩形用 `Shell_NotifyIconGetRect`。
"""

import ctypes
import sys
import time
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)
shell32 = ctypes.WinDLL("shell32", use_last_error=True)

HWND_MESSAGE = wintypes.HWND(-3)
TRAY_CLASS = "PeripheralBatteryTrayWnd"
ICON_ID = 1
WM_LBUTTONDOWN, WM_LBUTTONUP = 0x0201, 0x0202
WM_RBUTTONDOWN, WM_RBUTTONUP = 0x0204, 0x0205


class GUID(ctypes.Structure):
    _fields_ = [
        ("Data1", wintypes.DWORD),
        ("Data2", wintypes.WORD),
        ("Data3", wintypes.WORD),
        ("Data4", ctypes.c_ubyte * 8),
    ]


class NOTIFYICONIDENTIFIER(ctypes.Structure):
    _fields_ = [
        ("cbSize", wintypes.DWORD),
        ("hWnd", wintypes.HWND),
        ("uID", wintypes.UINT),
        ("guidItem", GUID),
    ]


class RECT(ctypes.Structure):
    _fields_ = [
        ("left", wintypes.LONG),
        ("top", wintypes.LONG),
        ("right", wintypes.LONG),
        ("bottom", wintypes.LONG),
    ]


def tray_hwnd() -> int:
    """托盘窗口是普通顶层窗口（0×0、不可见），按类名直接找。"""
    user32.FindWindowW.restype = wintypes.HWND
    user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
    hwnd = user32.FindWindowW(TRAY_CLASS, None)
    if not hwnd:
        raise SystemExit(f"找不到托盘窗口（类名 {TRAY_CLASS}）")
    return hwnd


def icon_rect() -> tuple[int, int, int, int]:
    hwnd = tray_hwnd()
    ident = NOTIFYICONIDENTIFIER()
    ident.cbSize = ctypes.sizeof(ident)
    ident.hWnd = hwnd
    ident.uID = ICON_ID
    rect = RECT()
    hr = shell32.Shell_NotifyIconGetRect(ctypes.byref(ident), ctypes.byref(rect))
    if hr != 0:
        raise SystemExit(f"Shell_NotifyIconGetRect 失败：0x{hr & 0xFFFFFFFF:08X}")
    return rect.left, rect.top, rect.right, rect.bottom


def click(button: str) -> None:
    left, top, right, bottom = icon_rect()
    x, y = (left + right) // 2, (top + bottom) // 2
    down, up = (WM_LBUTTONDOWN, WM_LBUTTONUP) if button == "left" else (WM_RBUTTONDOWN, WM_RBUTTONUP)
    user32.SetCursorPos(x, y)
    time.sleep(0.15)
    user32.mouse_event(down, 0, 0, 0, 0)
    time.sleep(0.05)
    user32.mouse_event(up, 0, 0, 0, 0)
    print(f"{button} click at ({x}, {y})")


if __name__ == "__main__":
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
    action = sys.argv[1] if len(sys.argv) > 1 else "rect"
    if action == "rect":
        print(icon_rect())
    elif action in {"left", "right"}:
        click(action)
    else:
        raise SystemExit(f"未知动作：{action}")
