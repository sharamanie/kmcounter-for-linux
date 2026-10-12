#!/usr/bin/env python3
"""抓取原始 evdev 按键事件（排查“某个键不计数”这类问题）。

用法：
    python3 tools/sniff_keys.py [秒数]        # 默认 60 秒
需要用户在 input 组（与程序本身的要求一致）。不抓取（EVIOCGRAB）也不影响正在运行的程序。

输出：每台设备上出现的按键（设备名 / 键码 / value）计数，以及修饰键的按下记录。
键码对照：evdev 的 KEY_* 编号（例如 LShift=42、RShift=54、RCtrl=97），
程序里的 `lin_code` 就是它；Windows 侧对应的是扫描码（RShift=0x36=54）。
"""
import os, select, struct, time, collections, glob, sys

DEVS = {}
for p in sorted(glob.glob('/dev/input/event*')):
    try:
        name = open(f'/sys/class/input/{os.path.basename(p)}/device/name').read().strip()
    except Exception:
        name = '?'
    try:
        DEVS[os.open(p, os.O_RDONLY | os.O_NONBLOCK)] = (p, name)
    except OSError:
        pass

print("监听设备:")
for p, n in DEVS.values():
    print(f"  {p}: {n}")

KEYS = {42: 'LShift', 54: 'RShift', 29: 'LCtrl', 97: 'RCtrl', 56: 'LAlt', 100: 'RAlt',
        125: 'LWin', 126: 'RWin', 58: 'CapsLock', 1: 'Esc'}
hits = collections.Counter()
detail = []
limit = float(sys.argv[1]) if len(sys.argv) > 1 else 60.0
t0 = time.time()
while time.time() - t0 < limit:
    ready, _, _ = select.select(list(DEVS.keys()), [], [], 1.0)
    for fd in ready:
        try:
            data = os.read(fd, 24 * 64)
        except BlockingIOError:
            continue
        for off in range(0, len(data) - 23, 24):
            _, _, ty, code, val = struct.unpack_from('qqHHi', data, off)
            if ty != 1:
                continue
            hits[(DEVS[fd][1], code, val)] += 1
            if code in KEYS and val == 1:
                detail.append(f"{time.strftime('%H:%M:%S')} {DEVS[fd][1]} → {KEYS[code]}(code {code}) 按下")

print(f"\n=== {limit:.0f} 秒内的按键统计（设备 / 键码 / value）===")
for (dev, code, val), n in sorted(hits.items(), key=lambda x: -x[1])[:30]:
    print(f"  {dev:<38} code={code:<4} value={val}  ×{n}")
print("\n=== 修饰键按下记录（最多 20 条）===")
print("\n".join(detail[-20:]) if detail else "  （这段时间内没有捕捉到修饰键按下）")
