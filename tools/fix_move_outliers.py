#!/usr/bin/env python3
"""清掉明显失真的「鼠标移动距离」。

背景：原版 KMCounter（AHK）的鼠标距离累加有个 bug —— 光标被程序强行挪动（全屏应用、
多显示器跳变）或它每 10 分钟重载钩子时"上一次坐标"过期，会把整屏跨度当成一次真实位移
累加进去。结果是少数日子记成几百公里，而正常日子只有 0.4~1.4 km，这些尖峰还会把
总计和趋势图彻底带偏（导入老数据后尤其明显）。

本工具只动「鼠标移动距离」这一项：
  - 超过阈值的日子：把该天的 move_px 置 0（若有分时明细，其各小时的 move_px 也置 0）；
  - 键盘敲击、每个按键的计数、鼠标按键/滚轮、总计里的非鼠标项：一律不动；
  - 总计里的 move_px 同步扣掉被清零的部分，保持「总计 = 各天之和」的口径。

用法：
    python3 tools/fix_move_outliers.py stats.json --dry-run     # 先看要改哪些天
    python3 tools/fix_move_outliers.py stats.json               # 实际写入（会自动备份）

参数：
    --cap-km N      超过 N 公里视为失真（默认 20）
    --screen-mm M   屏幕物理宽度毫米（默认 340，仅用于把像素换算成米来比较阈值）
    --screen-px P   屏幕像素宽度（默认 2560）
"""

import argparse
import json
import shutil
import sys
import statistics


def dump_like_app(obj, indent=0):
    """按程序的写法输出：整体 2 空格缩进，但 keys 数组写成单行紧凑数组
    （程序用 serde 的 RawValue 就是这么写的；否则 101 个数字铺 101 行，文件大三倍）。"""
    pad = " " * indent
    if isinstance(obj, dict):
        if not obj:
            return "{}"
        items = []
        for k, v in obj.items():
            if k == "keys" and isinstance(v, list):
                items.append(f'{pad}  "{k}": [' + ",".join(str(int(x)) for x in v) + "]")
            else:
                items.append(f'{pad}  "{k}": {dump_like_app(v, indent + 2)}')
        return "{\n" + ",\n".join(items) + f"\n{pad}}}"
    if isinstance(obj, list):
        if not obj:
            return "[]"
        return "[\n" + ",\n".join(f"{pad}  {dump_like_app(x, indent + 2)}" for x in obj) + f"\n{pad}]"
    if isinstance(obj, float):
        return json.dumps(obj)
    return json.dumps(obj, ensure_ascii=False)


def main() -> int:
    ap = argparse.ArgumentParser(description="清掉失真的鼠标移动距离")
    ap.add_argument("stats", help="stats.json 路径")
    ap.add_argument("--cap-km", type=float, default=20.0)
    ap.add_argument("--screen-mm", type=float, default=340.0)
    ap.add_argument("--screen-px", type=float, default=2560.0)
    ap.add_argument("--dry-run", action="store_true", help="只报告，不写入")
    args = ap.parse_args()

    # 与程序内 px_to_meters 一致：米 = 像素 × 屏宽毫米 / 屏宽像素 / 1000
    px_per_meter = 1000.0 * args.screen_px / args.screen_mm
    to_km = lambda px: px / px_per_meter / 1000.0
    cap_px = args.cap_km * 1000.0 * px_per_meter

    with open(args.stats) as f:
        store = json.load(f)

    days = store.get("days", {})
    vals = [d["mouse"]["move_px"] for d in days.values() if d.get("mouse")]
    if not vals:
        print("没有鼠标数据可检查。")
        return 1
    med = statistics.median(vals)

    hit = []
    for date in sorted(days):
        px = days[date].get("mouse", {}).get("move_px", 0.0)
        if px > cap_px:
            hit.append((date, px))
    if not hit:
        print(f"没有超过 {args.cap_km:g} km 的日子（中位数 {to_km(med):.2f} km）。")
        return 0

    removed = sum(px for _, px in hit)
    print(f"阈值 {args.cap_km:g} km；命中 {len(hit)} 天（全体中位数 {to_km(med):.2f} km）")
    for date, px in hit[:12]:
        print(f"  {date}: {to_km(px):.1f} km")
    if len(hit) > 12:
        print(f"  …以及另外 {len(hit) - 12} 天")
    total_before = store["total"]["mouse"]["move_px"]
    print(f"总计鼠标：{to_km(total_before):.0f} km → {to_km(total_before - removed):.0f} km")

    if args.dry_run:
        print("\n（--dry-run：未写入）")
        return 0

    bak = args.stats + ".bak"
    shutil.copy2(args.stats, bak)
    for date, _ in hit:
        day = days[date]
        day["mouse"]["move_px"] = 0.0
        for hour in day.get("hours", {}).values():
            hour.get("mouse", {})["move_px"] = 0.0
    store["total"]["mouse"]["move_px"] = total_before - removed
    with open(args.stats, "w") as f:
        f.write(dump_like_app(store) + "\n")
    print(f"\n已写入 {args.stats}（原文件备份为 {bak}）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
