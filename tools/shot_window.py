"""按窗口抓图（GPUI 窗口专用）。

为什么不用 `shot.py` 的整屏 BitBlt：GPUI 在 Windows 上走 DirectComposition，
画面不在 GDI 的表面里，从屏幕 DC 拷出来的只是它背后的桌面。`PrintWindow`
带 `PW_RENDERFULLCONTENT` 会让 DWM 把窗口内容画进我们给的 DC，这才是能截到
面板的唯一办法。

用法：
    python tools/shot_window.py out.png                      # 整窗
    python tools/shot_window.py out.png --crop 0 0 558 58    # 只要页眉
    python tools/shot_window.py out.png --zoom 3             # 放大 3 倍看细节
    python tools/shot_window.py out.png --title 别的标题      # 默认「外设电量显示」

所有坐标都是物理像素，脚本开头声明 DPI 感知。
"""

import ctypes
import sys
import time
from ctypes import wintypes

sys.path.insert(0, __file__.rsplit("\\", 1)[0])
import shot  # noqa: E402  （同目录的写 PNG 与窗口查找工具）

u, g = shot.user32, shot.gdi32
u.FindWindowW.restype = wintypes.HWND
u.PrintWindow.argtypes = [wintypes.HWND, wintypes.HDC, wintypes.UINT]
u.GetDC.restype = wintypes.HDC
g.CreateCompatibleDC.argtypes = [wintypes.HDC]
g.CreateCompatibleDC.restype = wintypes.HDC
g.CreateCompatibleBitmap.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
g.CreateCompatibleBitmap.restype = wintypes.HBITMAP
g.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
g.SelectObject.restype = wintypes.HGDIOBJ


def grab(hwnd: int) -> tuple[int, int, bytes]:
    """把整个客户区画进内存 DC，返回 (宽, 高, 自下而上的 BGR 原始字节)。"""
    rect = wintypes.RECT()
    u.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    u.GetClientRect(hwnd, ctypes.byref(rect))
    width = rect.right - rect.left
    height = rect.bottom - rect.top

    screen = u.GetDC(None)
    memory = g.CreateCompatibleDC(screen)
    bitmap = g.CreateCompatibleBitmap(screen, width, height)
    old = g.SelectObject(memory, bitmap)
    if not u.PrintWindow(hwnd, memory, 2):  # 2 = PW_RENDERFULLCONTENT
        raise SystemExit("PrintWindow 失败：窗口可能刚被隐藏")

    stride = (width * 3 + 3) & ~3
    buffer = ctypes.create_string_buffer(stride * height)
    info = shot.BITMAPINFO()
    info.bmiHeader.biSize = ctypes.sizeof(shot.BITMAPINFOHEADER)
    info.bmiHeader.biWidth = width
    info.bmiHeader.biHeight = height
    info.bmiHeader.biPlanes = 1
    info.bmiHeader.biBitCount = 24
    info.bmiHeader.biCompression = 0
    g.GetDIBits(memory, bitmap, 0, height, buffer, ctypes.byref(info), 0)

    g.SelectObject(memory, old)
    g.DeleteObject(bitmap)
    g.DeleteDC(memory)
    u.ReleaseDC(None, screen)
    return width, height, buffer.raw


def main() -> None:
    args = sys.argv[1:]
    if not args:
        raise SystemExit(__doc__)
    out = args[0]
    title = "外设电量显示"
    crop = None
    zoom = 1

    index = 1
    while index < len(args):
        if args[index] == "--title":
            title = args[index + 1]
            index += 2
        elif args[index] == "--crop":
            crop = tuple(int(value) for value in args[index + 1 : index + 5])
            index += 5
        elif args[index] == "--zoom":
            zoom = int(args[index + 1])
            index += 2
        else:
            raise SystemExit(f"不认识的参数：{args[index]}")

    u.SetProcessDPIAware()
    hwnd = u.FindWindowW(None, title)
    if not hwnd:
        raise SystemExit(f"找不到窗口：{title}")
    u.ShowWindow(hwnd, 5)
    u.SetForegroundWindow(hwnd)
    time.sleep(0.35)  # 给 GPUI 一帧时间把内容画出来

    width, height, raw = grab(hwnd)
    stride = (width * 3 + 3) & ~3

    if crop:
        cx, cy, cw, ch = crop
        cw = min(cw, width - cx)
        ch = min(ch, height - cy)
        # 原始字节是自下而上的：把裁剪区域换算成行号。
        rows = []
        for row in range(cy, cy + ch):
            source = height - 1 - row
            start = source * stride + cx * 3
            rows.append(raw[start : start + cw * 3])
        width, height = cw, ch
    else:
        rows = [
            raw[(height - 1 - row) * stride : (height - 1 - row) * stride + width * 3]
            for row in range(height)
        ]

    if zoom > 1:
        zoomed = []
        for line in rows:
            pixels = b"".join(line[i : i + 3] * zoom for i in range(0, len(line), 3))
            zoomed.extend([pixels] * zoom)
        rows = zoomed
        width, height = width * zoom, height * zoom

    rgb_rows = [
        b"".join(bytes((line[i + 2], line[i + 1], line[i])) for i in range(0, len(line), 3))
        for line in rows
    ]
    shot.write_png(out, width, height, rgb_rows)
    print(f"saved {out} {width}x{height}")


if __name__ == "__main__":
    main()
