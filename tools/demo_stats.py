#!/usr/bin/env python3
"""生成用于截图/演示的假 stats.json（含分时明细），不涉及任何真实数据。

用法：
    python3 tools/demo_stats.py /tmp/kmc-demo          # 生成 /tmp/kmc-demo/stats.json
    KMCOUNTER_SHOT_STATS=/tmp/kmc-demo/stats.json KMCOUNTER_SHOT_DIR=/tmp/kmc-shots \\
        cargo test --release --features softshot -- --ignored --nocapture render_readme_shots

生成 76 天数据：工作日强度高、周末偏低的日间节律，深夜只敲少数几个键
（方向键/空格等），这样分时热力图和 24 小时分布条在截图里能看出差别。
按键计数按最大余数法精确分配，保证每个小时的各键之和不小于该小时的总敲击数。
"""

import datetime
import json
import os
import random
import sys

N_KEYS = 101
DAYS = 76
SEED = 4242
# 一天 24 小时的相对强度（凌晨低、上午与下午两个高峰）
HOUR_FACTOR = [0.5, 0.3, 0.25, 0.25, 0.3, 0.5, 0.9, 1.2, 1.6, 2.0, 2.2, 2.3,
               1.6, 1.4, 1.8, 2.2, 2.1, 1.7, 1.3, 1.2, 1.1, 0.95, 0.8, 0.6]
# 深夜仍会按的键（方向键区 + 空格 + 少量字母）
NIGHT_KEYS = set(range(74, 85)) | {26, 48, 71, 82, 83, 84, 99}


def key_weights(rng):
    """按键频率画像：字母区常用、功能键少见"""
    w = [rng.random() ** 2.2 for _ in range(N_KEYS)]
    for i in (26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
              55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 68, 69, 70,
              80, 81, 82, 83, 84, 95, 96, 97, 98, 99, 100):
        w[i] *= 6.0
    for i in (0, 1, 2, 74, 75, 76, 77, 78, 79):
        w[i] *= 0.2
    return w


def spread(weights, total):
    """按权重把 total 精确分配到各键（最大余数法）"""
    s = sum(weights)
    if s <= 0 or total <= 0:
        return [0] * len(weights)
    exact = [total * w / s for w in weights]
    out = [int(x) for x in exact]
    for i in sorted(range(len(weights)), key=lambda i: exact[i] - out[i], reverse=True)[: total - sum(out)]:
        out[i] += 1
    return out


def dump(obj, indent=0):
    """手写序列化：按键数组写成单行紧凑数组，与程序输出一致"""
    pad = " " * indent
    if isinstance(obj, dict):
        if not obj:
            return "{}"
        items = [f'{pad}  "{k}": {dump(v, indent + 2)}' for k, v in obj.items()]
        return "{\n" + ",\n".join(items) + f"\n{pad}}}"
    if isinstance(obj, list):
        return "[" + ",".join(str(x) for x in obj) + "]"
    return json.dumps(obj)


def main(out_dir):
    rng = random.Random(SEED)
    base = key_weights(rng)
    today = datetime.date.today()
    start = today - datetime.timedelta(days=DAYS - 1)

    days = {}
    tot_keys = [0] * N_KEYS
    tot_mouse = {"move_px": 0.0, "lb": 0, "rb": 0, "mb": 0, "xb": 0, "wheel": 0, "hwheel": 0}
    tot_ks = 0
    tot_hours = {str(h): {"keystrokes": 0, "keys": [0] * N_KEYS,
                          "mouse": {"move_px": 0.0, "lb": 0, "rb": 0, "mb": 0, "xb": 0, "wheel": 0, "hwheel": 0}}
                 for h in range(24)}

    day = start
    while day <= today:
        peak = rng.uniform(6200, 8600)
        if day.weekday() >= 5:  # 周末略轻
            peak *= 0.7
        day_keys = [0] * N_KEYS
        day_mouse = {"move_px": 0.0, "lb": 0, "rb": 0, "mb": 0, "xb": 0, "wheel": 0, "hwheel": 0}
        hours = {}
        for h in range(24):
            ks = max(12, int(peak * HOUR_FACTOR[h] * rng.uniform(0.8, 1.2) / 12.0))
            w = [x * (1.0 + (rng.random() - 0.5) * 0.5) for x in base]
            if h < 7:
                for i in range(N_KEYS):
                    if i not in NIGHT_KEYS:
                        w[i] *= 0.02
            hk = spread(w, ks)
            mv = ks * rng.uniform(30.0, 55.0)
            hm = {"move_px": round(mv, 6), "lb": max(1, int(ks * 0.06)), "rb": int(ks * 0.012),
                  "mb": int(ks * 0.004), "xb": int(ks * 0.003), "wheel": int(ks * 0.07),
                  "hwheel": int(ks * 0.002)}
            hours[str(h)] = {"keystrokes": ks, "keys": hk, "mouse": hm}
            for i in range(N_KEYS):
                day_keys[i] += hk[i]
                tot_keys[i] += hk[i]
                tot_hours[str(h)]["keys"][i] += hk[i]
            for k, v in hm.items():
                if k == "move_px":
                    day_mouse[k] = round(day_mouse[k] + v, 6)
                    tot_mouse[k] = round(tot_mouse[k] + v, 6)
                    tot_hours[str(h)]["mouse"][k] = round(tot_hours[str(h)]["mouse"][k] + v, 6)
                else:
                    day_mouse[k] += v
                    tot_mouse[k] += v
                    tot_hours[str(h)]["mouse"][k] += v
            tot_hours[str(h)]["keystrokes"] += ks
        ks_day = sum(u["keystrokes"] for u in hours.values())
        tot_ks += ks_day
        days[day.strftime("%Y%m%d")] = {"keystrokes": ks_day, "keys": day_keys, "mouse": day_mouse, "hours": hours}
        day += datetime.timedelta(days=1)

    store = {
        "version": 1,
        "days": days,
        "total": {"keystrokes": tot_ks, "keys": tot_keys, "mouse": tot_mouse, "hours": tot_hours},
    }
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, "stats.json")
    with open(path, "w") as f:
        f.write(dump(store) + "\n")
    print(f"已生成 {DAYS} 天演示数据 → {path}（{os.path.getsize(path) / 1024:.1f} KiB）")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "/tmp/kmc-demo")
