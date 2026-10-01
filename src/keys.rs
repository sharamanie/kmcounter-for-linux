//! 键盘布局表：统一按键 ID、显示标签、Windows 扫描码、Linux evdev 键码、热力图几何位置。
//!
//! 按键身份沿用原版 KMCounter 的思路：用扫描码（含扩展位）区分主键区与数字键盘上的同名字键。
//! 几何单位：水平 1 单位 = key_w + key_spacing，垂直 1 行 = key_h + key_spacing。
//! 主键区总宽 15 单位；导航区 3 列；数字键盘区 4 列。

/// 一个按键在热力图中的定义
#[derive(Clone, Copy, Debug)]
pub struct KeyDef {
    /// 内部唯一名（作为统计数据的 JSON 键，必须唯一）
    pub name: &'static str,
    /// 界面显示标签（空则不显示文字）
    pub label: &'static str,
    /// Windows 扫描码（扩展键 = 0x100 | sc），0 表示无
    #[allow(dead_code)] // 仅 Windows 捕获使用
    pub win_sc: u16,
    /// Linux evdev 键码，0 表示无（仅 Linux 捕获使用）
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub lin_code: u16,
    /// 水平位置（单位）
    pub x: f32,
    /// 垂直行号（行单位）
    pub y: f32,
    /// 宽度（单位）
    pub w: f32,
    /// 高度（行单位），一般 1
    pub h: f32,
    /// 方向键箭头（用图形绘制，不用字体）
    pub arrow: Option<ArrowDir>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrowDir {
    Up,
    Down,
    Left,
    Right,
}

const fn k(
    name: &'static str,
    label: &'static str,
    win_sc: u16,
    lin_code: u16,
    x: f32,
    y: f32,
    w: f32,
) -> KeyDef {
    KeyDef { name, label, win_sc, lin_code, x, y, w, h: 1.0, arrow: None }
}

const fn ka(
    name: &'static str,
    win_sc: u16,
    lin_code: u16,
    x: f32,
    y: f32,
    arrow: ArrowDir,
) -> KeyDef {
    KeyDef { name, label: "", win_sc, lin_code, x, y, w: 1.0, h: 1.0, arrow: Some(arrow) }
}

const fn kh(name: &'static str, label: &'static str, win_sc: u16, lin_code: u16, x: f32, y: f32, w: f32, h: f32) -> KeyDef {
    KeyDef { name, label, win_sc, lin_code, x, y, w, h, arrow: None }
}

// F 行与 Esc 行之间的间隙：主键区宽 15 单位，13 个键 → 3 个间隙各 2/3 单位
const G: f32 = 2.0 / 3.0;

/// 全部布局按键。顺序即按键索引（KeyId）。
pub static KEYS: &[KeyDef] = &[
    // ---- 第一行：Esc + F1~F12 ----
    k("Esc", "Esc", 1, 1, 0.0, 0.0, 1.0),
    k("F1", "F1", 59, 59, 1.0 + G, 0.0, 1.0),
    k("F2", "F2", 60, 60, 2.0 + G, 0.0, 1.0),
    k("F3", "F3", 61, 61, 3.0 + G, 0.0, 1.0),
    k("F4", "F4", 62, 62, 4.0 + G, 0.0, 1.0),
    k("F5", "F5", 63, 63, 5.0 + 2.0 * G, 0.0, 1.0),
    k("F6", "F6", 64, 64, 6.0 + 2.0 * G, 0.0, 1.0),
    k("F7", "F7", 65, 65, 7.0 + 2.0 * G, 0.0, 1.0),
    k("F8", "F8", 66, 66, 8.0 + 2.0 * G, 0.0, 1.0),
    k("F9", "F9", 67, 67, 9.0 + 3.0 * G, 0.0, 1.0),
    k("F10", "F10", 68, 68, 10.0 + 3.0 * G, 0.0, 1.0),
    k("F11", "F11", 87, 87, 11.0 + 3.0 * G, 0.0, 1.0),
    k("F12", "F12", 88, 88, 12.0 + 3.0 * G, 0.0, 1.0),
    // ---- 第二行 ----
    k("Backquote", "`", 41, 41, 0.0, 1.0, 1.0),
    k("D1", "1", 2, 2, 1.0, 1.0, 1.0),
    k("D2", "2", 3, 3, 2.0, 1.0, 1.0),
    k("D3", "3", 4, 4, 3.0, 1.0, 1.0),
    k("D4", "4", 5, 5, 4.0, 1.0, 1.0),
    k("D5", "5", 6, 6, 5.0, 1.0, 1.0),
    k("D6", "6", 7, 7, 6.0, 1.0, 1.0),
    k("D7", "7", 8, 8, 7.0, 1.0, 1.0),
    k("D8", "8", 9, 9, 8.0, 1.0, 1.0),
    k("D9", "9", 10, 10, 9.0, 1.0, 1.0),
    k("D0", "0", 11, 11, 10.0, 1.0, 1.0),
    k("Minus", "-", 12, 12, 11.0, 1.0, 1.0),
    k("Equal", "=", 13, 13, 12.0, 1.0, 1.0),
    k("Backspace", "Backspace", 14, 14, 13.0, 1.0, 2.0),
    // ---- 第三行 ----
    kh("Tab", "Tab", 15, 15, 0.0, 2.0, 1.5, 1.0),
    k("Q", "q", 16, 16, 1.5, 2.0, 1.0),
    k("W", "w", 17, 17, 2.5, 2.0, 1.0),
    k("E", "e", 18, 18, 3.5, 2.0, 1.0),
    k("R", "r", 19, 19, 4.5, 2.0, 1.0),
    k("T", "t", 20, 20, 5.5, 2.0, 1.0),
    k("Y", "y", 21, 21, 6.5, 2.0, 1.0),
    k("U", "u", 22, 22, 7.5, 2.0, 1.0),
    k("I", "i", 23, 23, 8.5, 2.0, 1.0),
    k("O", "o", 24, 24, 9.5, 2.0, 1.0),
    k("P", "p", 25, 25, 10.5, 2.0, 1.0),
    k("BracketL", "[", 26, 26, 11.5, 2.0, 1.0),
    k("BracketR", "]", 27, 27, 12.5, 2.0, 1.0),
    kh("Backslash", "\\", 43, 43, 13.5, 2.0, 1.5, 1.0),
    // ---- 第四行 ----
    kh("CapsLock", "CapsLock", 58, 58, 0.0, 3.0, 1.75, 1.0),
    k("A", "a", 30, 30, 1.75, 3.0, 1.0),
    k("S", "s", 31, 31, 2.75, 3.0, 1.0),
    k("D", "d", 32, 32, 3.75, 3.0, 1.0),
    k("F", "f", 33, 33, 4.75, 3.0, 1.0),
    k("G", "g", 34, 34, 5.75, 3.0, 1.0),
    k("H", "h", 35, 35, 6.75, 3.0, 1.0),
    k("J", "j", 36, 36, 7.75, 3.0, 1.0),
    k("K", "k", 37, 37, 8.75, 3.0, 1.0),
    k("L", "l", 38, 38, 9.75, 3.0, 1.0),
    k("Semicolon", ";", 39, 39, 10.75, 3.0, 1.0),
    k("Quote", "'", 40, 40, 11.75, 3.0, 1.0),
    kh("Enter", "Enter", 28, 28, 12.75, 3.0, 2.25, 1.0),
    // ---- 第五行 ----
    kh("LShift", "Shift", 42, 42, 0.0, 4.0, 2.25, 1.0),
    k("Z", "z", 44, 44, 2.25, 4.0, 1.0),
    k("X", "x", 45, 45, 3.25, 4.0, 1.0),
    k("C", "c", 46, 46, 4.25, 4.0, 1.0),
    k("V", "v", 47, 47, 5.25, 4.0, 1.0),
    k("B", "b", 48, 48, 6.25, 4.0, 1.0),
    k("N", "n", 49, 49, 7.25, 4.0, 1.0),
    k("M", "m", 50, 50, 8.25, 4.0, 1.0),
    k("Comma", ",", 51, 51, 9.25, 4.0, 1.0),
    k("Period", ".", 52, 52, 10.25, 4.0, 1.0),
    k("Slash", "/", 53, 53, 11.25, 4.0, 1.0),
    kh("RShift", "Shift", 54, 54, 12.25, 4.0, 2.75, 1.0),
    // ---- 第六行 ----
    kh("LCtrl", "Ctrl", 29, 29, 0.0, 5.0, 1.25, 1.0),
    kh("LWin", "Win", 347, 125, 1.25, 5.0, 1.25, 1.0),
    kh("LAlt", "Alt", 56, 56, 2.5, 5.0, 1.25, 1.0),
    kh("Space", "Space", 57, 57, 3.75, 5.0, 6.25, 1.0),
    kh("RAlt", "Alt", 312, 100, 10.0, 5.0, 1.25, 1.0),
    kh("RWin", "Win", 348, 126, 11.25, 5.0, 1.25, 1.0),
    kh("Menu", "Menu", 349, 139, 12.5, 5.0, 1.25, 1.0),
    kh("RCtrl", "Ctrl", 285, 97, 13.75, 5.0, 1.25, 1.0),
    // ---- 导航区（x 由绘制时加偏移）----
    k("Insert", "Insert", 338, 110, 0.0, 1.0, 1.0),
    k("Home", "Home", 327, 102, 1.0, 1.0, 1.0),
    k("PageUp", "PageUp", 329, 104, 2.0, 1.0, 1.0),
    k("Delete", "Delete", 339, 111, 0.0, 2.0, 1.0),
    k("End", "End", 335, 107, 1.0, 2.0, 1.0),
    k("PageDown", "PageDown", 337, 109, 2.0, 2.0, 1.0),
    ka("Up", 328, 103, 1.0, 4.0, ArrowDir::Up),
    ka("Left", 331, 105, 0.0, 5.0, ArrowDir::Left),
    ka("Down", 336, 108, 1.0, 5.0, ArrowDir::Down),
    ka("Right", 333, 106, 2.0, 5.0, ArrowDir::Right),
    // ---- 数字键盘区（x 由绘制时加偏移）----
    k("NumLock", "NumLock", 325, 69, 0.0, 1.0, 1.0),
    k("NumSlash", "/", 309, 98, 1.0, 1.0, 1.0),
    k("NumAsterisk", "*", 55, 55, 2.0, 1.0, 1.0),
    k("NumMinus", "-", 74, 74, 3.0, 1.0, 1.0),
    k("Num7", "7", 71, 71, 0.0, 2.0, 1.0),
    k("Num8", "8", 72, 72, 1.0, 2.0, 1.0),
    k("Num9", "9", 73, 73, 2.0, 2.0, 1.0),
    kh("NumPlus", "+", 78, 78, 3.0, 2.0, 1.0, 2.0),
    k("Num4", "4", 75, 75, 0.0, 3.0, 1.0),
    k("Num5", "5", 76, 76, 1.0, 3.0, 1.0),
    k("Num6", "6", 77, 77, 2.0, 3.0, 1.0),
    k("Num1", "1", 79, 79, 0.0, 4.0, 1.0),
    k("Num2", "2", 80, 80, 1.0, 4.0, 1.0),
    k("Num3", "3", 81, 81, 2.0, 4.0, 1.0),
    kh("NumEnter", "Enter", 284, 96, 3.0, 4.0, 1.0, 2.0),
    kh("Num0", "0", 82, 82, 0.0, 5.0, 2.0, 1.0),
    k("NumDot", ".", 83, 83, 2.0, 5.0, 1.0),
    // ---- 导航区顶部的三个系统键（原版键盘图没有它们，这里补上）----
    // 它们**按控制键区管理**：画在导航块顶部一行（见 block_of），统计也计入控制键区。
    // 追加在数组末尾：`keys` 数组按索引存盘，插到中间会让已有统计错位
    k("PrtScr", "PrtSc", 311, 99, 0.0, 0.0, 1.0),
    k("ScrollLock", "ScrLk", 70, 70, 1.0, 0.0, 1.0),
    k("Pause", "Pause", 69, 119, 2.0, 0.0, 1.0),
];

/// 布局按键总数
pub const N_KEYS: usize = KEYS.len();

/// 功能键区（Esc + F1~F12）结束位置
pub const FUNC_END: usize = 13;
/// 主键区按键数（KEYS 前缀区间 [0, MAIN_END)）
pub const MAIN_END: usize = 74;
/// 导航区结束（区间 [MAIN_END, NAV_END) 为导航区，之后为数字键盘区）
pub const NAV_END: usize = 84;
/// 数字键盘区结束（区间 [NAV_END, NUM_END) 为数字键盘区；之后是后补的三个系统键）
pub const NUM_END: usize = 101;

/// 统计用的键盘分区（与原版键盘图的划分一致）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// 功能键区：Esc + F1~F12
    Function,
    /// 主键盘区：字母数字，以及 Shift / Ctrl / Alt / 空格等
    Main,
    /// 控制键区：Insert~方向键，以及后补的 PrtScr / ScrollLock / Pause
    Control,
    /// 数字键盘区
    Numpad,
}

/// 统计表里的分区顺序
pub const ZONES: [Zone; 4] = [Zone::Main, Zone::Function, Zone::Control, Zone::Numpad];

/// 控制键区在 KEYS 里的两段区间：导航区本体 [MAIN_END, NAV_END)，
/// 以及后补的 PrtScr / ScrollLock / Pause [NUM_END, N_KEYS)（它们按控制键区管理，
/// 画在导航块顶部一行，统计也计入控制键区）。
pub const CONTROL_RANGES: [std::ops::Range<usize>; 2] = [MAIN_END..NAV_END, NUM_END..N_KEYS];

/// 按键所属分区（绘制分块也以它为准，见 [`block_of`]）
pub fn zone_of(i: usize) -> Zone {
    if i < FUNC_END {
        Zone::Function
    } else if i < MAIN_END {
        Zone::Main
    } else if CONTROL_RANGES.iter().any(|r| r.contains(&i)) {
        Zone::Control
    } else {
        Zone::Numpad
    }
}

/// 绘制块：功能键区与主键盘区画在同一块；控制键区的两段都画在导航块（PrtScr/ScrLk/Pause 在它顶部一行）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    Main,
    Control,
    Numpad,
}

/// 按键画在哪一块 —— 直接由分区决定，避免“分区”和“绘制”两套规则走偏
pub fn block_of(i: usize) -> Block {
    match zone_of(i) {
        Zone::Function | Zone::Main => Block::Main,
        Zone::Control => Block::Control,
        Zone::Numpad => Block::Numpad,
    }
}

/// 某分区的按键计数合计
pub fn zone_sum(z: Zone, count: impl Fn(usize) -> u64) -> u64 {
    (0..N_KEYS).filter(|i| zone_of(*i) == z).map(count).sum()
}

/// 按键内部名 → 索引（迁移旧数据用）
pub fn idx_by_name(name: &str) -> Option<usize> {
    KEYS.iter().position(|k| k.name == name)
}

/// 主键区宽度（单位）
pub const MAIN_UNITS: f32 = 15.0;
/// 导航区宽度（单位）
pub const NAV_UNITS: f32 = 3.0;
/// 数字键盘区宽度（单位）
pub const NUM_UNITS: f32 = 4.0;
/// 主键区底部行数
pub const MAIN_ROWS: f32 = 6.0;

/// Windows 扫描码 → 按键索引
#[allow(dead_code)] // 仅 Windows 捕获使用
pub fn idx_by_win_sc(sc: u16) -> Option<usize> {
    KEYS.iter().position(|k| k.win_sc == sc)
}

/// Linux evdev 键码 → 按键索引（仅 Linux 捕获使用）
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn idx_by_lin_code(code: u16) -> Option<usize> {
    KEYS.iter().position(|k| k.lin_code == code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_and_codes_unique() {
        let mut names = HashSet::new();
        let mut win = HashSet::new();
        let mut lin = HashSet::new();
        for k in KEYS {
            assert!(k.win_sc != 0, "{} 缺少 win_sc", k.name);
            assert!(k.lin_code != 0, "{} 缺少 lin_code", k.name);
            assert!(names.insert(k.name), "重复 name: {}", k.name);
            assert!(win.insert(k.win_sc), "重复 win_sc {} ({})", k.win_sc, k.name);
            assert!(lin.insert(k.lin_code), "重复 lin_code {} ({})", k.lin_code, k.name);
        }
    }

    #[test]
    fn rows_aligned() {
        // 每一行按键右边缘应与 15 单位对齐（F 行对齐到 Backspace 右缘，即 12+3G+1 = 15）
        for (row, expected) in [(0.0, 12.0 + 3.0 * G + 1.0), (1.0, 15.0), (2.0, 15.0), (3.0, 15.0), (4.0, 15.0), (5.0, 15.0)] {
            let right = KEYS.iter().filter(|k| k.y == row && k.x < MAIN_UNITS)
                .map(|k| k.x + k.w)
                .fold(0.0_f32, f32::max);
            assert!((right - expected).abs() < 1e-4, "第 {} 行右缘 {} != {}", row, right, expected);
        }
    }

    #[test]
    fn lookup_works() {
        assert_eq!(idx_by_win_sc(57), Some(KEYS.iter().position(|k| k.name == "Space").unwrap()));
        assert_eq!(idx_by_lin_code(57), idx_by_win_sc(57));
        // 数字键盘 1 与主键盘 1 必须是不同的键
        let num1 = KEYS.iter().position(|k| k.name == "Num1").unwrap();
        let d1 = KEYS.iter().position(|k| k.name == "D1").unwrap();
        assert_ne!(num1, d1);
    }

    #[test]
    fn block_ranges() {
        // 主键区 [0, MAIN_END)：x 覆盖整个 15 单位宽度
        assert!(KEYS[..MAIN_END].iter().all(|k| k.x < MAIN_UNITS));
        assert_eq!(KEYS[..MAIN_END].last().map(|k| k.name), Some("RCtrl"));
        // 导航区 [MAIN_END, NAV_END)：10 个键，3 列
        let nav = &KEYS[MAIN_END..NAV_END];
        assert_eq!(nav.len(), 10);
        assert!(nav.iter().all(|k| k.x <= 2.0 && k.y >= 1.0));
        // 数字键盘区 [NAV_END, NUM_END)：17 个键，4 列
        let num = &KEYS[NAV_END..NUM_END];
        assert_eq!(num.len(), 17);
        assert!(num.iter().all(|k| k.x <= 3.0 && k.y >= 1.0));
        // 后补的系统键 [NUM_END,..)：3 个，画在导航块顶部一行（x 0..2、y=0）
        let sys = &KEYS[NUM_END..];
        assert_eq!(sys.len(), 3);
        assert!(sys.iter().all(|k| k.y == 0.0 && k.x < NAV_UNITS));
    }

    #[test]
    fn zones_partition_all_keys() {        // 每个键恰好属于一个分区（四个分区合起来覆盖全部按键）
        let mut hits = vec![0usize; N_KEYS];
        for z in ZONES {
            for (i, h) in hits.iter_mut().enumerate() {
                if zone_of(i) == z {
                    *h += 1;
                }
            }
        }
        assert!(hits.iter().all(|h| *h == 1), "分区没有恰好覆盖：{hits:?}");
        // 分区范围：功能键区 13、主键区 61、控制键区 13（含 3 个系统键）、数字键区 17
        let count = |z: Zone| (0..N_KEYS).filter(|i| zone_of(*i) == z).count();
        assert_eq!(count(Zone::Function), 13);
        assert_eq!(count(Zone::Main), MAIN_END - FUNC_END);
        assert_eq!(count(Zone::Control), (NAV_END - MAIN_END) + 3);
        assert_eq!(count(Zone::Numpad), NUM_END - NAV_END);
        // 系统键归控制键区
        for name in ["PrtScr", "ScrollLock", "Pause"] {
            let i = idx_by_name(name).unwrap();
            assert_eq!(zone_of(i), Zone::Control, "{name} 应归控制键区");
        }
        // 抽查：Esc/F12 属功能键区，空格属主键盘区，NumEnter 属数字键区
        assert_eq!(zone_of(idx_by_name("Esc").unwrap()), Zone::Function);
        assert_eq!(zone_of(idx_by_name("F12").unwrap()), Zone::Function);
        assert_eq!(zone_of(idx_by_name("Space").unwrap()), Zone::Main);
        assert_eq!(zone_of(idx_by_name("NumEnter").unwrap()), Zone::Numpad);
        // zone_sum 与逐键求和一致
        let counts: Vec<u64> = (0..N_KEYS).map(|i| i as u64).collect();
        let expect: u64 = (MAIN_END..NAV_END).map(|i| i as u64).sum::<u64>()
            + (NUM_END..N_KEYS).map(|i| i as u64).sum::<u64>();
        assert_eq!(zone_sum(Zone::Control, |i| counts[i]), expect);
    }

    /// 绘制块由分区决定：三个系统键跟导航区一样画在控制块（导航块顶部一行）
    #[test]
    fn block_follows_zone() {
        let block = |n: &str| block_of(idx_by_name(n).unwrap());
        assert_eq!(block("Esc"), Block::Main);
        assert_eq!(block("F1"), Block::Main);
        assert_eq!(block("Space"), Block::Main);
        for n in ["Insert", "Home", "PageUp", "Delete", "End", "PageDown", "Up", "Left", "Down", "Right"] {
            assert_eq!(block(n), Block::Control, "{n} 应在控制块");
        }
        // 后补的三个系统键：与导航区同一块（所以它们画在导航块顶部一行）
        for n in ["PrtScr", "ScrollLock", "Pause"] {
            assert_eq!(block(n), Block::Control, "{n} 应由控制键区管理");
            assert_eq!(zone_of(idx_by_name(n).unwrap()), Zone::Control);
        }
        for n in ["NumLock", "Num5", "Num0", "NumEnter"] {
            assert_eq!(block(n), Block::Numpad, "{n} 应在数字键盘块");
        }
        // 控制块的两段合起来正好是控制键区
        let ctrl: Vec<usize> = CONTROL_RANGES.iter().flat_map(|r| r.clone()).collect();
        assert_eq!(ctrl.len(), 13);
        assert!(ctrl.iter().all(|i| zone_of(*i) == Zone::Control));
    }
}
