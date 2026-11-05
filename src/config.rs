//! TOML 配置文件。布局默认值与原版 KMCounter 一致（键宽 52 / 键高 45 / 键间距 2 / 区域水平间距 10）；
//! 历史保留天数默认 365 天（原版默认 9999，见 `history_days_clamped`）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct LayoutCfg {
    /// 键宽（像素）
    pub key_w: u32,
    /// 键高（像素）
    pub key_h: u32,
    /// 键间距（像素）
    pub key_spacing: u32,
    /// 区域水平间距（像素）
    pub h_spacing: u32,
    /// 键盘配列: "104" | "98" | "87" | "75" | "68" | "64" | "60"
    pub keyboard_layout: String,
}

impl Default for LayoutCfg {
    fn default() -> Self {
        LayoutCfg {
            key_w: 52,
            key_h: 45,
            key_spacing: 2,
            h_spacing: 10,
            keyboard_layout: "104".into(),
        }
    }
}

/// KPS 监控键数量上限 = 模型里的全部物理键个数（" + "KEYS.len()）。
///
/// 为什么是 104：与主界面键盘图完全对齐 —— 用户能在键盘上看到的每一颗键
/// 都应该能被监控（Esc、F1-F12、整个主键区、导航区、小键盘、系统键）。
/// 以前取 60 是拍脑袋留余量，实际上键盘图里就有 104 个可选键。
pub const KPS_KEY_MAX: u8 = 104;

/// 单行最多几个键（未显式写换行时的自动折行宽度）。
///
/// 21 是 104 键盘的物理宽度：数字行与 Tab 行都是 21 个键（
/// 含 Ins/Home/PgUp 与小键盘那一组）。取 21 才能把真实布局完整排下来。
pub const KPS_ROW_MAX: usize = 21;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Config {
    /// 界面语言："auto" | "zh" | "en"
    pub language: String,
    /// 历史数据保留天数（最小 0）
    pub history_days: u32,
    /// 点击窗口关闭按钮时：true = 隐藏到托盘继续统计，false = 直接退出
    pub close_to_tray: bool,
    /// (Linux/KDE) 自动写入 KWin 窗口规则：不在任务栏与 Alt-Tab 中显示（像 QQ 一样只留托盘）
    pub skip_taskbar: bool,
    /// (Linux/KDE) 从托盘呼出窗口时保持置顶。Wayland/KWin 下程序化激活不稳定，
    /// 置顶是唯一可靠的“置于最前”方式；不喜欢窗口压在其他窗口上可设为 false（呼出可能不置前）
    pub keep_on_top: bool,
    /// 显示器物理宽度（毫米），0 = 自动检测
    pub screen_w_mm: u32,
    /// 显示器物理高度（毫米），0 = 自动检测
    pub screen_h_mm: u32,
    /// 主显示器像素宽度，0 = 自动检测（仅 Windows 能自动，Linux 默认 1920）
    pub screen_px_w: u32,
    /// 中文字体文件路径，空 = 自动查找
    pub font_path: String,
    /// 开机启动（由托盘菜单/设置管理）
    pub autostart: bool,
    /// 首次运行的欢迎横幅是否已显示过
    pub welcome_shown: bool,
    /// 独立 APM 悬浮窗是否显示
    pub hud_apm_show: bool,
    /// APM 滑动窗口大小（秒），默认 60
    pub apm_window_secs: u32,
    /// 独立 KPS 悬浮窗是否显示
    pub hud_kps_show: bool,
    /// KPS 滑动窗口大小（毫秒），默认 1000
    pub kps_window_ms: u32,
    /// KPS 监控按键列表（逗号隔开，如 "Q, W, E, R"）
    pub kps_keys: String,
    /// KPS 监控键数量（1~KPS_KEY_MAX）：键表按这个数量维护，默认键位取自 QWERTY 序列
    pub kps_key_count: u8,
    /// **按键数记忆的键表**：`{6: "A,S,D,F,G,H", 10: "..."}`。
    ///
    /// 用户把键数从 6 改到 12 再改回 6 时，希望 6 键的那套键位还在。
    /// 键的 key 是 `u8` 转字符串（TOML 的 table key 只能是字符串，用 BTreeMap 保证写盘顺序稳定）。
    /// 空表 = 旧版本配置，行为回退到「只保留当前一份」。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kps_keys_by_count: std::collections::BTreeMap<String, String>,
    /// KPS 悬浮窗是否显示按键累计次数
    pub kps_show_hits: bool,
    /// KPS 悬浮窗是否显示 Max 峰值
    pub kps_show_max: bool,
    /// KPS 悬浮窗是否显示平均 KPS（自上次重置起）
    pub kps_show_avg: bool,
    /// KPS 悬浮窗：每个键也显示它自己的 KPS
    pub kps_show_key_kps: bool,
    /// KPS 悬浮窗缩放（百分比，60~400；对齐 osu-kps 的 Zoom 菜单）
    pub kps_scale: u16,
    /// APM 胶囊条缩放（百分比，60~400；对齐 KPS 的缩放）
    pub apm_scale: u16,
    /// KPS 键格是否让**最后一行**的键平分整行宽度（参照用户自定义的分行布局）
    pub kps_auto_width: bool,
    /// KPS 计算方式："hard"（严格按窗口计数）| "sensitive"（指数平滑，更跟手）
    pub kps_method: String,
    /// KPS 悬浮窗是否显示按键格子
    pub kps_show_buttons: bool,
    /// KPS 悬浮窗是否显示统计信息（当前 / Max / 平均）
    pub kps_show_stats: bool,
    /// KPS 悬浮窗是否显示 KPS 随时间变化的图
    pub kps_show_graph: bool,
    /// KPS 图的时长（秒，10~300）
    pub kps_graph_secs: u32,
    /// KPS 悬浮窗是否显示**按键雨**：每次击键在键格上方升起一条轨迹
    /// （对齐 JipperKeyViewer 的 RainLayer；与下面的 KPS 图相互独立，可只开其一）
    pub kps_show_rain: bool,
    /// 键雨上升速度：**走完整个轨道要多少毫秒**（200~20000；越小越快）。
    /// 旧语义是像素/秒 —— 高度一改速度观感就变，改按轨道时间后同一速度值在任何
    /// 高度下观感一致（对齐 JipperKeyViewer 的「按秒配置」习惯）。
    pub kps_rain_speed: u16,
    /// 键雨轨道高度（像素，16~400；0 = 自动取 96px）
    pub kps_rain_height: u16,
    /// 键雨条宽占键格宽的比例（百分比，20~100）。默认 72 = 略窄于键格。
    pub kps_rain_width_pct: u8,
    /// 键雨不透明度（0~100）
    pub kps_rain_opacity: u8,
    /// 键雨渐隐带占轨道高度的百分比（5~90）。轨道顶部往下这一段里，雨条越靠上越透明。
    pub kps_rain_fade_pct: u8,
    /// 键雨颜色（RGB，默认纯白）。键雨**不跟随 KPS 热度变色** —— 雨条是「这次按压的
    /// 运动轨迹」，跟着手速变色会和键格自身的发光混在一起，看不出层次。
    pub kps_rain_color: [u8; 3],
    /// 键格按压弹跳：按下瞬间放大再回弹（对齐 JipperKeyViewer 的 KeyShapeLayer）
    pub kps_key_bounce: bool,
    /// 按组 KPS 分组定义：一行一组，格式 `组名=A,S,D,F`（组名可省略）
    pub kps_groups: String,
    /// KPS 悬浮窗是否显示分组 KPS 行
    pub kps_show_groups: bool,
    /// APM 悬浮窗背景不透明度（0~100）
    pub apm_hud_opacity: u8,
    /// APM 悬浮窗：拖动后若超出屏幕自动推回（对齐 osu-kps 的同名选项；默认开）
    pub apm_keep_in_screen: bool,
    /// KPS 悬浮窗：同上
    pub kps_keep_in_screen: bool,
    /// 悬浮窗固定位置：开启后**禁止拖动**（两个悬浮窗共用）。
    /// 默认关：保持现有的“拖到哪记到哪”体验。
    /// 注意与 `*_hud_pos`（位置记忆）的区别：那个是自动记录，这个是是否允许拖动。
    pub hud_lock_pos: bool,
    /// 悬浮窗是否**始终置顶**（在所有窗口之上）。
    /// 默认开；关掉后悬浮窗会像普通窗口一样可被其他窗口盖住。
    pub hud_always_on_top: bool,
    /// 上次退出时的 APM 悬浮窗位置（窗口左上角，逻辑像素；自动记录、重启恢复，无设置项）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apm_hud_pos: Option<(f32, f32)>,
    /// 上次退出时的 KPS 悬浮窗位置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kps_hud_pos: Option<(f32, f32)>,
    /// KPS 悬浮窗背景不透明度（0~100）
    pub kps_hud_opacity: u8,
    /// APM 悬浮窗是否显示键盘敲击数
    pub apm_show_keys: bool,
    /// APM 悬浮窗是否显示鼠标点击数
    pub apm_show_clicks: bool,
    /// APM 悬浮窗是否显示鼠标移动距离
    pub apm_show_dist: bool,
    /// 鼠标移动距离阈值（公里/小时）：某个小时超过则**该小时整体作废**
    /// （原始数值仍记录在案并打上标记，只是不计入当日/总计；0 = 关闭）
    pub limit_move_km: f64,
    /// 鼠标点击总数阈值（左+右+中+侧键，次/小时）：超过则该小时作废（0 = 关闭）
    pub limit_clicks: u64,
    /// 键盘敲击总数阈值（次/小时）：超过则该小时作废（0 = 关闭）
    pub limit_keystrokes: u64,
    pub layout: LayoutCfg,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            language: "auto".into(),
            history_days: 365,
            close_to_tray: true,
            skip_taskbar: false,
            keep_on_top: false,
            screen_w_mm: 0,
            screen_h_mm: 0,
            screen_px_w: 0,
            font_path: String::new(),
            autostart: false,
            hud_apm_show: false,
            apm_window_secs: 60,
            hud_kps_show: false,
            kps_window_ms: 1000,
            kps_keys: "Q, W, E, R".into(),
            kps_key_count: 4,
            kps_keys_by_count: std::collections::BTreeMap::new(),
            kps_show_hits: true,
            kps_show_max: true,
            kps_show_avg: true,
            kps_show_key_kps: true,
            kps_scale: 100,
            apm_scale: 100,
            kps_auto_width: false,
            kps_method: "hard".into(),
            kps_show_buttons: true,
            kps_show_stats: true,
            kps_show_graph: true,
            kps_graph_secs: 300,
            kps_show_rain: false,
            kps_rain_speed: 1200,
            kps_rain_height: 96,
            kps_rain_width_pct: 72,
            kps_rain_opacity: 85,
    kps_rain_fade_pct: 35,
            kps_rain_color: [255, 255, 255],
            kps_key_bounce: true,
            kps_groups: String::new(),
            kps_show_groups: false,
            apm_hud_opacity: 94,
            apm_keep_in_screen: true,
            kps_keep_in_screen: true,
            hud_lock_pos: false,
            hud_always_on_top: true,
            apm_hud_pos: None,
            kps_hud_pos: None,
            kps_hud_opacity: 94,
            apm_show_keys: true,
            apm_show_clicks: true,
            apm_show_dist: true,
            limit_move_km: 20.0,
            limit_clicks: 0,
            limit_keystrokes: 0,
            welcome_shown: false,
            layout: LayoutCfg::default(),
        }
    }
}

/// KPS 默认键位顺序：键盘上字母键从左到右、从上到下的顺序（qwerty 第一排往后）
pub const DEFAULT_KPS_KEY_SEQUENCE: &str = "qwertyuiopasdfghjklzxcvbnm";

/// KPS 键格布局里的一个格子
#[derive(Clone, Debug, PartialEq)]
pub struct KpsKeyCell {
    /// 键名（空串 = 空位：占一格但不显示、不统计）
    pub name: String,
    /// 宽度（单位是"格"；`1.5H` / `4H` 这种写法就是 1.5 / 4）
    pub width: f32,
    /// 高度（单位是"行"，默认 1）。`2x2H` = 宽 2 格、高 2 行。
    /// 跨行时**只在第一行写一次**，后面那几行自动让位。
    pub height: f32,
}

/// KPS 键格布局：按用户写的分行，或默认每行最多 12 个自动换行
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KpsKeyLayout {
    pub rows: Vec<Vec<KpsKeyCell>>,
    /// 最宽一行的总格数（自适应键宽按它平分）
    pub cols: f32,
}

impl KpsKeyLayout {
    /// 所有非空格的键名（按布局顺序）
    pub fn names(&self) -> Vec<String> {
        self.rows
            .iter()
            .flat_map(|r| r.iter())
            .filter(|c| !c.name.is_empty())
            .map(|c| c.name.clone())
            .collect()
    }
}

/// 解析一格：`H` / `1.5H` / `4H` / 空
/// 把 1.5 / 2.0 / 6.25 这种宽度写成最短形式（去掉无意义的 ".0"）。
fn trim_float(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// 解析「宽度 / 高度 / 间距」用的数值：支持小数与**分数**。
///
/// - `1.5` / `0.5` —— 小数
/// - `2/3` —— 分数（2 除以 3），用于 104 键盘 F 区那种「三个空隙均分 2 格」的场景
///
/// 为什么需要分数：104 键盘的 F 区里，Esc→F1、F4→F5、F8→F9 这三段空隙
/// 在真实键盘上并不相等，但加起来正好 2 格。写 `0.6667` 会累积误差，
/// 写 `2/3` 则是精确值。
///
/// 返回 (`消耗掉的字符数`, 数值)。解析失败返回 None。
fn parse_number(s: &str) -> Option<(usize, f32)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut seen_dot = false;
    let mut seen_slash = false;
    while i < s.len() {
        let c = bytes[i] as char;
        if c.is_ascii_digit() {
            i += 1;
        } else if c == '.' && !seen_dot && !seen_slash {
            seen_dot = true;
            i += 1;
        } else if c == '/' && !seen_slash {
            seen_slash = true;
            i += 1;
        } else {
            break;
        }
    }
    if i == 0 {
        return None;
    }
    let text = &s[..i];
    let val = if let Some((a, b)) = text.split_once('/') {
        let a: f32 = a.parse().ok()?;
        let b: f32 = b.parse().ok()?;
        if b <= 0.0 {
            return None;
        }
        a / b
    } else {
        text.parse::<f32>().ok()?
    };
    if !val.is_finite() {
        return None;
    }
    Some((i, val))
}

fn parse_kps_cell(item: &str) -> KpsKeyCell {
    let t = item.trim();
    if t.is_empty() {
        return KpsKeyCell { name: String::new(), width: 1.0, height: 1.0 };
    }
    // 逗号是**分隔符**，想监控逗号键本身就不能直接写 ","。
    // 这里把它的其他形态统一换回 "," 再交给 normalize_kps_key：
    //   <  《  〈  —— 都是逗号键。
    // 同样地，句号键的 > 》 〉 。 也归一到 "."。
    let t: &str = match t {
        "<" | "《" | "〈" => ",",
        ">" | "》" | "〉" | "。" => ".",
        _ => t,
    };
    let t = t.trim();

    // **纯数字 / 纯分数 = 间距**：F,0.5,G 或 F,2/3,G
    //
    // 和「数字前缀 = 宽度」分开：0.5G 是「宽 0.5 格的 G」（有键名），
    // 而单独一个 0.5（没有键名）是「一道 0.5 格宽的缝」。
    if let Some((used, gap)) = parse_number(t) {
        if used == t.len() && gap > 0.0 {
            return KpsKeyCell {
                name: String::new(),
                width: gap.clamp(0.1, 12.0),
                height: 1.0,
            };
        }
    }

    // 带键名：开头是数值（宽度），可选 `x` + 数值（高度）
    if let Some((used, w)) = parse_number(t) {
        if used < t.len() {
            let rest = t[used..].trim_start();
            // 形如 2x2H
            if let Some(after_x) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
                if let Some((h_used, h)) = parse_number(after_x) {
                    let name = after_x[h_used..].trim_start().to_string();
                    if !name.is_empty() {
                        return KpsKeyCell {
                            name,
                            width: w.clamp(0.5, 12.0),
                            height: h.clamp(1.0, 12.0),
                        };
                    }
                }
            }
            // 只有宽度：1.5H / 2/3G
            if (0.1..=12.0).contains(&w) && !rest.is_empty() {
                return KpsKeyCell {
                    name: rest.to_string(),
                    width: w,
                    height: 1.0,
                };
            }
        }
    }
    KpsKeyCell { name: t.to_string(), width: 1.0, height: 1.0 }
}

impl Config {
    /// 默认键位的前 n 个，**按键盘物理顺序从第一排到最后一排**：
    /// Esc, F1, F2, … F12, `, 1, 2, … 0, -, =, Backspace, Tab, Q, W, …
    ///
    /// 以前用的是 "qwertyuiopasdfghjklzxcvbnm"（字母键优先），但用户想监控 F 键、方向键、
    /// 小键盘等时就得手写，不如直接按键盘图的顺序给。
    ///
    /// 按「当前配列」的真实键盘顺序取前 n 个键名（Esc, F1, F2, …）。
    ///
    /// 为什么要按配列：104 / 98 / 87 … 的实际布局并不一样（小键盘、导航区的位置与是否存在都不同），
    /// 主界面选了 98 配列，HUD 就不该把 104 才有的键给列进来。
    /// `LayoutProfile::keys` 本身已按配列排好序（行 → x），直接取其 idx 就行。
    pub fn default_kps_keys_for_layout(layout_id: &str, n: usize) -> Vec<String> {
        let profile = crate::keys::layout_profile(layout_id);
        let n = n.clamp(1, profile.keys.len());
        profile
            .keys
            .iter()
            .take(n)
            .map(|k| {
                // 优先用配列自己的显示名（紧凑配列里 ` 位置就是 Esc），
                // 没有覆盖时用键本身的 name。这样用户看到的名字与
                // 主界面键盘图完全一致。
                match k.label_override {
                    Some(l) => l.to_string(),
                    None => crate::keys::KEYS[k.idx].name.to_string(),
                }
            })
            .collect()
    }

    /// **迁移：把旧版单行键表换成配列的真实多行布局。**
    ///
    /// 旧版只能写成一长串，靠「每行 21 个硬折行」排版 —— 排出来是
    /// 每行堆砌、宽键不对、方向键跑到别的位置。
    ///
    /// 只在两个前提同时满足时才迁移，保证不会动用户自定义的键表：
    /// 1. 键表是**单行**（没有换行）；
    /// 2. 它的键名序列与该配列的默认顺序**完全一致**（说明没自定义过）。
    /// 返回 true 表示确实发生了迁移（调用方需立即写盘）。
    pub fn migrate_flat_kps_keys(&mut self) -> bool {
        // 已经是多行了：不动
        if self.kps_keys.contains('\n') {
            return false;
        }
        let cur = self.kps_key_layout().names();
        if cur.is_empty() {
            return false;
        }
        // 判断“没自定义过”用**键集合**而不是顺序：
        //
        // 旧版默认序列按“字母优先”排（qwertyuiop…），后来又改过一次按
        // 键盘物理顺序，两种都不一定与当前配列的生成顺序完全一致。
        // 但只要**键名集合**对得上，就说明用户没增删过键，可以安全重排。
        let want = Self::default_kps_keys_for_layout(&self.layout.keyboard_layout, cur.len());
        if cur.len() != want.len() {
            return false;
        }
        let mut a: Vec<String> = cur.iter().map(|s| s.to_lowercase()).collect();
        let mut b: Vec<String> = want.iter().map(|s| s.to_lowercase()).collect();
        a.sort();
        b.sort();
        if a != b {
            return false; // 键集合不一样：用户自定义过，不碰
        }
        let new_text = Self::kps_keys_text_for_layout(&self.layout.keyboard_layout);
        if new_text != self.kps_keys {
            log::info!("键表迁移：单行 → 配列 {} 的真实行布局", self.layout.keyboard_layout);
            self.kps_keys = new_text;
            true
        } else {
            false
        }
    }

    /// 把配列的**真实几何**转成键表文本（换行 = 分行、宽度倍数 = 宽键）。
    ///
    /// 这就是「在 HUD 上按真实键盘排列」的实现：
    /// - **分行**：直接用 `LayoutKey.y`（键盘图的行号）归类；
    /// - **顺序**：每行按 `LayoutKey.x` 升序；
    /// - **宽键**：`LayoutKey.w` ≠ 1 时写成前缀（如 `1.5Tab`、`6.25Space`）。
    ///
    /// 为什么不用横坐标差算缝：真实键盘的 F 区、导航区、小键盘之间有大段空隙。
    /// 若把它们也用空位补齐，键表会长到无法阅读；而只保留行内顺序与宽键，
    /// 观感上已经就是真实键盘。要精确对齐某个空位时，用户自己写 "," 占位即可。
    pub fn kps_keys_text_for_layout(layout_id: &str) -> String {
        let profile = crate::keys::layout_profile(layout_id);
        let mut rows: Vec<(f32, Vec<(f32, &crate::keys::LayoutKey)>)> = Vec::new();
        for k in profile.keys.iter() {
            match rows.iter_mut().find(|(y, _)| (*y - k.y).abs() < 0.01) {
                Some((_, list)) => list.push((k.x, k)),
                None => rows.push((k.y, vec![(k.x, k)])),
            }
        }
        rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut lines: Vec<String> = Vec::with_capacity(rows.len());
        for (_, mut list) in rows {
            list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let items: Vec<String> = list
                .iter()
                .map(|(_, k)| {
                    let name = match k.label_override {
                        Some(l) => l.to_string(),
                        None => crate::keys::KEYS[k.idx].name.to_string(),
                    };
                    // 宽度倍数只在真的不是 1 格时才写，免得键表到处是 "1.0"
                    if (k.w - 1.0).abs() < 0.01 {
                        name
                    } else {
                        format!("{}{}", trim_float(k.w), name)
                    }
                })
                .collect();
            lines.push(items.join(", "));
        }
        lines.join("\n")
    }

    /// 默认键位的前 n 个（不带配列信息时的退化版本：按 104 配列）。
    pub fn default_kps_keys_for(n: usize) -> Vec<String> {
        Self::default_kps_keys_for_layout("104", n)
    }

    /// 当前键表是否还是"默认序列的前 N 个"（用户没自定义过）
    pub fn kps_keys_is_default(&self) -> bool {
        let cur = self.kps_key_layout().names();
        let want = Self::default_kps_keys_for_layout(&self.layout.keyboard_layout, cur.len());
        cur.iter().map(|s| s.to_lowercase()).eq(want.iter().map(|s| s.to_lowercase()))
    }

    /// 解析「监控按键」为键格布局：
    /// - 逗号分隔；**换行 = 分行**（不写换行时每行最多 12 个自动换行）；
    /// - 空项（两个逗号挨着） = 空位，占一格（可以用来把后面的键挪到指定列）；
    /// - 数字前缀 = 宽度倍数：`1.5H` = H 占 1.5 格、`4H` = 4 格；
    /// - 行尾多余的逗号忽略。
    pub fn kps_key_layout(&self) -> KpsKeyLayout {
        let mut rows: Vec<Vec<KpsKeyCell>> = Vec::new();
        for line in self.kps_keys.split('\n') {
            let trimmed = line.trim_end();
            if trimmed.trim().is_empty() {
                continue; // 空行忽略
            }
            let cells: Vec<KpsKeyCell> = trimmed.split(',').map(parse_kps_cell).collect();
            // **行尾逗号 = 末尾空一格**（用户要求）。
            //
            // 以前尾部空项被直接丢掉，于是 "H,," 和 "H" 一样。
            // 但真实键盘布局里「最后一个键右侧留白」很常见（导航区、方向键上方），
            // 保留它们才能把键挤到指定列。
            // 只删掉空行、保留真实空位。
            if cells.is_empty() {
                continue;
            }
            rows.push(cells);
        }
        // 没写换行 → 默认每行最多 12 个
        if rows.len() == 1 && rows[0].len() > KPS_ROW_MAX {
            let flat = rows.pop().unwrap_or_default();
            let mut wrapped: Vec<Vec<KpsKeyCell>> = Vec::new();
            let mut cur: Vec<KpsKeyCell> = Vec::new();
            for c in flat {
                cur.push(c);
                if cur.len() == KPS_ROW_MAX {
                    wrapped.push(std::mem::take(&mut cur));
                }
            }
            if !cur.is_empty() {
                wrapped.push(cur);
            }
            rows = wrapped;
        }
        if rows.is_empty() {
            rows.push(
                Self::default_kps_keys_for(self.kps_key_count.max(1) as usize)
                    .into_iter()
                    .map(|name| KpsKeyCell { name, width: 1.0, height: 1.0 })
                    .collect(),
            );
        }
        // **跨行占位**：height = 2 的键格在下一行同列插一个空占位，
        // 使后面的键不会左移。只在**声明行写一次**，不要求用户重复写。
        //
        // 为什么要做这件事：真实键盘里小键盘的 + 与 Enter 是竖向 2 行高的，
        // 它们右侧的键并不会因此左移。
        let snapshot = rows.clone();
        for (ri, row) in snapshot.iter().enumerate() {
            if ri + 1 >= rows.len() {
                break;
            }
            let mut col = 0usize;
            for cell in row.iter() {
                let units = cell.width.max(0.1).ceil() as usize;
                if cell.height > 1.0 {
                    for k in 1..(cell.height.round() as usize) {
                        // 在 rows[ri + k] 的 col 位置插入空占位
                        if let Some(target) = rows.get_mut(ri + k) {
                            if col <= target.len() {
                                let blank = KpsKeyCell {
                                    name: String::new(),
                                    width: cell.width,
                                    height: 1.0,
                                };
                                // 直接插在第 col 格。
                                //
                                // 跨行键占的就是「声明行那一列」的下方，
                                // 所以层下行那一格本来就该是空的。
                                // 不要去「跳过非空项」：那会跳到行尾，
                                // 反而让后面的键错位。
                                let at = col.min(target.len());
                                target.insert(at, blank);
                            }
                        }
                    }
                }
                col += units;
            }
        }

        let cols = rows
            .iter()
            .map(|r| r.iter().map(|c| c.width).sum::<f32>())
            .fold(1.0f32, f32::max);
        // 自适应键宽：**每一行**各自平分整行宽度。
        //
        // 旧行为只处理最后一行，结果多行布局时上面几行还是 1 格宽、
        // 只有最后一行被撑满，长短不一、看着极不整齐（用户反馈）。
        // 现在每一行都撑满：一行 n 个键 → 每个键占 cols/n 格，
        // 所有行左右边缘对齐，观感统一。
        // 哪一行自己写过宽度倍数（如 1.5H、4X）就不干预那一行。
        if self.kps_auto_width {
            for row in rows.iter_mut() {
                let n = row.len() as f32;
                if n <= 0.0 {
                    continue;
                }
                let explicit = row.iter().any(|c| (c.width - 1.0).abs() > f32::EPSILON);
                if explicit {
                    continue;
                }
                for c in row.iter_mut() {
                    c.width = cols / n;
                }
            }
        }
        KpsKeyLayout { rows, cols }
    }

    /// 键数变化后调整键表：
    /// - 还是默认表 → 直接按新数量重排（所以"6 键"就是 q, w, e, r, t, y）；
    /// - 自定义表（用户设置过键位）→ 保留前 N 个，不足的从默认序列补（跳过已经用过的）。
    pub fn resize_kps_keys(&mut self) {
        let n = (self.kps_key_count.clamp(1, KPS_KEY_MAX)) as usize;
        // ① 先把**当前**键表存进「按键数记忆」。
        //
        // 这一步必须在探索新键表之前：否则用户把 6 改成 12 时，
        // 第九个键起就被覆盖了，改回 6 也找不回原来那套。
        // 这里必须用「上一份键表的键数」作 key：
        // 用户把 6 改成 12 时，当前键表还是 6 键那份，直接用它的名字个数做 key。
        let prev_names = self.kps_key_layout().names();
        if !prev_names.is_empty() {
            let cur = self.kps_keys.clone();
            if !cur.trim().is_empty() {
                self.kps_keys_by_count.insert(prev_names.len().to_string(), cur);
            }
        }

        // ② 如果这个键数之前存过，**直接恢复**（这就是「随时切换」的体验）
        if let Some(saved) = self.kps_keys_by_count.get(&n.to_string()).cloned() {
            let names = Self::parse_names(&saved);
            if names.len() >= n {
                self.kps_keys = saved;
                self.kps_key_count = n as u8;
                return;
            }
        }

        // ③ 没存过：按旧规则推导一份
        let cur = self.kps_key_layout().names();
        let result = if cur.is_empty() || self.kps_keys_is_default() {
            Self::default_kps_keys_for_layout(&self.layout.keyboard_layout, n).join(", ")
        } else {
            let mut list: Vec<String> = cur.into_iter().take(n).collect();
            let used: std::collections::BTreeSet<String> =
                list.iter().map(|s| s.to_lowercase()).collect();
            for c in DEFAULT_KPS_KEY_SEQUENCE.chars() {
                if list.len() >= n {
                    break;
                }
                if !used.contains(&c.to_string()) {
                    list.push(c.to_uppercase().to_string());
                }
            }
            list.join(", ")
        };
        self.kps_keys = result;
        self.kps_key_count = n as u8;
    }

    /// 从键表字符串里取出真实键名（去掉空位 / 宽度前缀）。
    fn parse_names(keys: &str) -> Vec<String> {
        keys.split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                // 宽度前缀："1.5H" / "4X" → 取后面的键名
                let t = s.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
                if t.is_empty() { s.to_string() } else { t.to_string() }
            })
            .collect()
    }

    pub fn load(path: &Path) -> (Config, bool) {
        crate::fsutil::clean_stale_tmp(path);
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(mut c) => {
                    // 旧版的键表是**单行**字符串，只能靠每行 21 个硬折行，
                    // 排出来是“每行堆砌”。配列真实布局需要换行与宽键倍数。
                    // 这里做一次性迁移：只要键表恰好是该配列的默认顺序
                    // （说明用户没自定义过），就换成多行真实布局。
                    // 迁移若生效，**立即写盘**。
                    // 不能等退出时的 save_all：那路径保存的是 cfg_shared，
                    // 而用户可能直接 kill / 异常退出，迁移结果就丢了。
                    if c.migrate_flat_kps_keys() {
                        if let Err(e) = c.save(path) {
                            log::warn!("键表迁移后写盘失败: {e}");
                        }
                    }
                    (c, false)
                }
                Err(e) => {
                    // 解析失败：先把原文件备份下来再退回默认配置。
                    // 不能当成“首次运行”（否则欢迎横幅重弹，且退出时会把默认配置写回去、
                    // 用户那处笔误就被静默吞掉了）。
                    let bak = crate::fsutil::quarantine_corrupt(path);
                    match bak {
                        Some(bak) => log::warn!(
                            "配置解析失败({e})，原文件已留档为 {}，本次使用默认配置",
                            bak.display()
                        ),
                        None => log::warn!("配置解析失败({e})，使用默认配置（留档失败）"),
                    }
                    (Config::default(), false)
                }
            },
            Err(_) => {
                let c = Config::default();
                let _ = c.save(path);
                (c, true) // 第二个返回值：是否为首次运行
            }
        }
    }

    /// 只读加载（命令行 `--stats`/`--export` 用）：不创建文件、解析失败用默认值。
    /// 避免只读命令在数据目录里留下副作用文件。
    pub fn load_readonly(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    pub fn kps_key_list(&self) -> Vec<String> {
        let list: Vec<String> = self.kps_keys
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if list.is_empty() {
            vec!["Z".into(), "X".into(), "M1".into(), "M2".into()]
        } else {
            list
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let body = toml::to_string_pretty(self).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let header = "\
# KMCounter-rs 配置文件
# language: 界面语言, auto | zh | en
# history_days: 历史数据保留天数
# close_to_tray: 点窗口关闭按钮时隐藏到托盘(true) 还是直接退出(false)
# skip_taskbar: (Linux/KDE) 自动写入 KWin 规则，不在任务栏/Alt-Tab 显示，只留托盘
# keep_on_top: (Linux/KDE) 从托盘呼出时是否置顶；false 时靠 KWin 激活置前，可能被其他窗口挡住
# screen_w_mm / screen_h_mm: 显示器物理尺寸(毫米), 0 = 自动检测
# screen_px_w: 主显示器像素宽度, 0 = 自动检测
# font_path: 中文字体文件路径, 留空自动查找
# limit_move_km: 某小时鼠标移动超过该值（公里）则该小时作废（原始值仍记录，只是不计入当日/总计；0 = 关闭）
# limit_clicks / limit_keystrokes: 某小时鼠标点击 / 键盘敲击超过该值则作废（0 = 关闭）
# [layout]: 键盘热力图的尺寸(像素)
# apm_scale / kps_scale: 悬浮窗缩放百分比; *_keep_in_screen: 拖出屏幕自动推回;
# hud_lock_pos: 固定位置(禁止拖动悬浮窗),默认 false;
# hud_always_on_top: 悬浮窗始终置顶(在所有窗口之上),默认 true;
# *_hud_pos: 位置记忆(自动记录,勿手改)
";
        // 原子写：GUI 与后台 saver 可能同时保存配置，直接截断写会互相覆盖
        crate::fsutil::atomic_write(path, format!("{header}{body}").as_bytes())
    }

    pub fn history_days_clamped(&self) -> u32 {
        // 上限放宽到 10000 天（约 27 年）：原版默认就是 9999，导入老数据时别把它截短
        self.history_days.min(10000)
    }
}

/// 数据目录：
/// 1. `--config` 指定的路径优先；
/// 2. 否则用**可执行文件同级目录**（便携，与原版 KMCounter 一致：程序和数据放在一起，
///    双系统共用同一块盘时可以指向同一份数据）；
/// 3. 程序目录不可写时（例如装到 /usr/bin 或 Program Files）回退到用户配置目录。
pub fn data_dir(custom: Option<PathBuf>) -> PathBuf {
    if let Some(p) = custom {
        return p;
    }
    static RESOLVED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    RESOLVED.get_or_init(resolve_data_dir).clone()
}

/// 用户配置目录（旧版本/不可写时的回退位置）
pub fn user_data_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("kmcounter")
}

fn resolve_data_dir() -> PathBuf {
    let legacy = user_data_dir();
    let Some(dir) = exe_dir() else { return legacy };
    // 程序目录里的 datadir.txt 可以把数据指到别处（程序放系统盘、数据放移动盘/双系统共享盘）
    if let Some(target) = read_datadir_marker(&dir) {
        if ensure_dir_writable(&target) {
            log::debug!("数据目录: {}（由程序旁的 datadir.txt 指定）", target.display());
            return target;
        }
        log::warn!(
            "datadir.txt 指向的目录不可用：{}，改用程序目录 {}",
            target.display(),
            dir.display()
        );
    }
    if dir_writable(&dir) {
        migrate_legacy(&dir, &legacy);
        log::debug!("数据目录: {}（程序同级，便携模式）", dir.display());
        dir
    } else {
        log::info!(
            "程序目录 {} 不可写，数据仍存放在 {}（可用 --config 指定其他位置）",
            dir.display(),
            legacy.display()
        );
        legacy
    }
}

/// 读取程序目录里的 `datadir.txt`：取第一行非空且非 `#` 注释的内容作为数据目录。
/// 支持 `~` 开头的路径；相对路径按「程序目录」解析。
fn read_datadir_marker(exe_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(exe_dir.join("datadir.txt")).ok()?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;
    let path = match line {
        "~" => dirs::home_dir()?,
        _ => match line.strip_prefix("~/") {
            Some(rest) => dirs::home_dir()?.join(rest),
            None => {
                let p = PathBuf::from(line);
                if p.is_absolute() {
                    p
                } else {
                    exe_dir.join(p)
                }
            }
        },
    };
    Some(path)
}

/// 确保目录存在且可写（datadir.txt 指向的目录可能还没建）
fn ensure_dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() && std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    dir_writable(dir)
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// 用「创建再删除一个探针文件」判断目录是否可写（比权限位可靠，Windows/Linux 通用）
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".kmcounter-write-test-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 首次切到程序目录时，把用户配置目录里的数据搬过来（同一台机器升级后无缝衔接）
fn migrate_legacy(new: &Path, old: &Path) {
    if new == old {
        return;
    }
    let has_new = new.join("stats.json").exists() || new.join("config.toml").exists();
    let has_old = old.join("stats.json").exists() || old.join("config.toml").exists();
    if has_new || !has_old {
        return;
    }
    for name in ["stats.json", "config.toml"] {
        let (src, dst) = (old.join(name), new.join(name));
        if !src.exists() {
            continue;
        }
        let ok = std::fs::rename(&src, &dst).is_ok()
            || (std::fs::copy(&src, &dst).is_ok() && std::fs::remove_file(&src).is_ok());
        if ok {
            log::info!("已把 {name} 从 {} 迁移到 {}", old.display(), new.display());
        } else {
            log::warn!("迁移 {name} 失败，仍从 {} 读取", src.display());
        }
    }
    // 运行期的小文件不必搬
    let _ = std::fs::remove_file(old.join("show.request"));
    let _ = std::fs::remove_file(old.join("kmcounter.lock"));
}

#[cfg(test)]
mod tests {
    /// 分数写法的间距：F,2/3,G 表示留 2/3 格。
    ///
    /// 用于 104 键盘 F 区那种「三个空隙均分 2 格」的场景 ——
    /// 写 0.6667 会累积误差，2/3 是精确值。
    #[test]
    fn fraction_gap_is_supported() {
        let mut cfg = Config::default();
        cfg.kps_keys = "F, 2/3, G".into();
        let l = cfg.kps_key_layout();
        let row = &l.rows[0];
        assert_eq!(row.len(), 3);
        assert_eq!(row[1].name, "");
        assert!((row[1].width - 2.0 / 3.0).abs() < 1e-6, "宽度应为 2/3，实际 {}", row[1].width);
    }

    /// 分数也能当宽度前缀：2/3G = 宽 2/3 格的 G。
    #[test]
    fn fraction_prefix_is_width() {
        let mut cfg = Config::default();
        cfg.kps_keys = "2/3G, H".into();
        let l = cfg.kps_key_layout();
        assert_eq!(l.rows[0][0].name, "G");
        assert!((l.rows[0][0].width - 2.0 / 3.0).abs() < 1e-6);
    }

    /// 三段的 2/3 + 2/3 + 2/3 加起来正好 2 格（不留浮点残渣）。
    #[test]
    fn three_thirds_sum_to_two() {
        let mut cfg = Config::default();
        cfg.kps_keys = "A, 2/3, 2/3, 2/3, B".into();
        let l = cfg.kps_key_layout();
        let total: f32 = l.rows[0].iter().map(|c| c.width).sum();
        assert!((total - 4.0).abs() < 1e-5, "A + 2格 + B = 4，实际 {total}");
    }


    /// 纯数字项 = 间距：F,0.5,G 表示 F 与 G 之间留 0.5 格。
    #[test]
    fn bare_number_is_gap_between_keys() {
        let mut cfg = Config::default();
        cfg.kps_keys = "F, 0.5, G".into();
        let l = cfg.kps_key_layout();
        let row = &l.rows[0];
        assert_eq!(row.len(), 3, "应是 F / 间距 / G 三格：{:?}", row.iter().map(|c| &c.name).collect::<Vec<_>>());
        assert_eq!(row[0].name, "F");
        assert_eq!(row[1].name, "", "中间那格应是空位");
        assert!((row[1].width - 0.5).abs() < 1e-3, "间距宽度应为 0.5，实际 {}", row[1].width);
        assert_eq!(row[2].name, "G");
    }

    /// 数字键本身不受影响：单独写 1 仍是 D1 键而不是间距。
    #[test]
    fn digit_key_still_parses_as_key() {
        let mut cfg = Config::default();
        cfg.kps_keys = "A, D1, B".into();
        let l = cfg.kps_key_layout();
        assert_eq!(l.rows[0][1].name, "D1", "D1 应仍是键名");
    }

    /// 有键名的数字前缀仍是宽度（0.5G 不是间距）。
    #[test]
    fn number_prefix_with_name_is_width() {
        let mut cfg = Config::default();
        cfg.kps_keys = "0.5G, H".into();
        let l = cfg.kps_key_layout();
        assert_eq!(l.rows[0][0].name, "G", "0.5G 应是键 G");
        assert!((l.rows[0][0].width - 0.5).abs() < 1e-3);
    }


    /// 新布局语法：行尾逗号 = 末尾空位。
    #[test]
    fn trailing_comma_makes_tail_gap() {
        let mut cfg = Config::default();
        cfg.kps_keys = "A, B,".into();
        let l = cfg.kps_key_layout();
        assert_eq!(l.rows[0].len(), 3, "行尾逗号应产生第 3 个空位：{:?}", l.rows[0]);
        assert_eq!(l.rows[0][2].name, "");
    }

    /// 新布局语法：`2x2H` = 宽 2 格、高 2 行。
    #[test]
    fn cell_can_span_two_rows() {
        let mut cfg = Config::default();
        cfg.kps_keys = "2x2H, A\nX, B".into();
        let l = cfg.kps_key_layout();
        let c = &l.rows[0][0];
        assert_eq!(c.name, "H");
        assert!((c.width - 2.0).abs() < 1e-3, "宽应为 2，实际 {}", c.width);
        assert!((c.height - 2.0).abs() < 1e-3, "高应为 2，实际 {}", c.height);
        // 第二行应在同列插入空占位，A 不会左移
        assert!(
            l.rows[1].iter().take(2).any(|x| x.name.is_empty()),
            "第二行应有占位空格：{:?}",
            l.rows[1].iter().map(|x| &x.name).collect::<Vec<_>>()
        );
    }

    /// 只写一个数字仍然是宽度（向后兼容 1.5H）。
    #[test]
    fn single_number_is_still_width() {
        let mut cfg = Config::default();
        cfg.kps_keys = "1.5Tab, Q".into();
        let l = cfg.kps_key_layout();
        assert!((l.rows[0][0].width - 1.5).abs() < 1e-3);
        assert!((l.rows[0][0].height - 1.0).abs() < 1e-3, "高度应保持 1");
    }

    /// 键格布局语法：换行=分行、逗号后无空格也行、空项=空位、数字前缀=宽度倍数、自适应键宽平分最后一行
    #[test]
    /// 自适应键宽必须作用于**每一行**，而不只是最后一行。
    ///
    /// 用户反馈：多行布局时上面几行还是 1 格宽、只有最后一行被撑满，
    /// 长短不一很难看。现在要求每行各自平分，使所有行左右边缘对齐。
    #[test]
    fn auto_width_applies_to_every_row() {
        let mut cfg = Config::default();
        // 三行，分别 10 / 10 / 4 个键；最宽行是 10 格
        cfg.kps_keys = "Q,W,E,R,T,Y,U,I,O,P
A,S,D,F,G,H,J,K,L,
Z,X,C,V".into();
        cfg.kps_auto_width = true;
        let layout = cfg.kps_key_layout();
        assert_eq!(layout.rows.len(), 3);
        let total = layout.cols;
        assert!((total - 10.0).abs() < 1e-3, "最宽行应为 10 格，实际 {total}");
        for (i, row) in layout.rows.iter().enumerate() {
            let sum: f32 = row.iter().map(|c| c.width).sum();
            assert!(
                (sum - total).abs() < 1e-3,
                "第 {i} 行宽度和应为 {total}，实际 {sum}（自适应未生效）"
            );
            // 行内各键等宽
            let w0 = row[0].width;
            assert!(row.iter().all(|c| (c.width - w0).abs() < 1e-3), "行内各键应等宽");
        }
    }

    /// 写了宽度倍数的行不被自适应干预。
    #[test]
    fn auto_width_skips_rows_with_explicit_widths() {
        let mut cfg = Config::default();
        cfg.kps_keys = "Q,W,E,R
1.5H,J".into();
        cfg.kps_auto_width = true;
        let layout = cfg.kps_key_layout();
        // 第二行写了 1.5H，不应被改写
        let row1: Vec<f32> = layout.rows[1].iter().map(|c| c.width).collect();
        assert!((row1[0] - 1.5).abs() < 1e-3, "1.5H 应保留，实际 {row1:?}");
        // 第一行没写，应被平分到 cols
        let sum0: f32 = layout.rows[0].iter().map(|c| c.width).sum();
        assert!((sum0 - layout.cols).abs() < 1e-3, "第一行应平分到 {}，实际 {sum0}", layout.cols);
    }

    fn kps_key_layout_parses_rows_blanks_and_widths() {
        let mut cfg = Config::default();
        // 用户写的四行
        cfg.kps_keys = "E,R,Q,W,T,\nY,U,I,O,P,\nA,S,D,F,G,\nH,J".into();
        let l = cfg.kps_key_layout();
        assert_eq!(l.rows.len(), 4, "四行：{l:?}");
        assert_eq!(l.rows.iter().map(|r| r.len()).collect::<Vec<_>>(), vec![5, 5, 5, 2]);
        assert_eq!(l.names().len(), 17, "共 17 个键（行尾逗号不算空位）");
        assert_eq!(l.cols, 5.0, "最宽一行 5 格");

        // 空项 = 空位：把 H、J 挪到第 3、4 列
        cfg.kps_keys = "E,R,Q,W,T,\nY,U,I,O,P,\nA,S,D,F,G,\n,,H,J".into();
        let l = cfg.kps_key_layout();
        assert_eq!(l.rows[3][0].name, "");
        assert_eq!(l.rows[3][1].name, "");
        assert_eq!(l.rows[3][2].name, "H");
        assert_eq!(l.rows[3][3].name, "J");
        assert_eq!(l.names().len(), 17, "空位不参与统计");
        assert_eq!(l.cols, 5.0, "空位也占宽度");

        // 宽度倍数：1.5H / 4H
        cfg.kps_keys = "E,R,Q,W,T,\nY,U,I,O,P,\nA,S,D,F,G,\n1.5H,J".into();
        let l = cfg.kps_key_layout();
        assert_eq!(l.rows[3][0].width, 1.5);
        assert_eq!(l.rows[3][0].name, "H");
        assert_eq!(l.rows[3][1].width, 1.0);
        cfg.kps_keys = "E,R,Q,W,T,\nY,U,I,O,P,\nA,S,D,F,G,\n4H,J".into();
        assert_eq!(cfg.kps_key_layout().rows[3][0].width, 4.0);

        // 自适应键宽（可开启）：最后一行平分整行 → H、J 各 2.5 格
        cfg.kps_keys = "E,R,Q,W,T,\nY,U,I,O,P,\nA,S,D,F,G,\nH,J".into();
        cfg.kps_auto_width = true;
        let l = cfg.kps_key_layout();
        assert_eq!(l.rows[3][0].width, 2.5);
        assert_eq!(l.rows[3][1].width, 2.5);
        assert_eq!(l.rows[0][0].width, 1.0, "其它行不受影响");
        // 该行自己写过倍数时不干预
        cfg.kps_keys = "E,R,Q,W,T,\nY,U,I,O,P,\nA,S,D,F,G,\n1.5H,J".into();
        let l = cfg.kps_key_layout();
        assert_eq!(l.rows[3][0].width, 1.5, "写了倍数就按用户写的");

        // 不写换行 → 每行最多 12 个自动换行；纯数字项是数字键本身，不当倍数
        cfg.kps_auto_width = false;
        cfg.kps_keys = (0..26).map(|i| format!("K{i}")).collect::<Vec<_>>().join(", ");
        assert_eq!(
            cfg.kps_key_layout().rows.iter().map(|r| r.len()).collect::<Vec<_>>(),
            vec![12, 12, 2]
        );
        cfg.kps_keys = "1, 2, M1, 1.5H".into();
        let l = cfg.kps_key_layout();
        assert_eq!(l.names(), vec!["1", "2", "M1", "H"]);
        assert_eq!(l.rows[0][3].width, 1.5);
    }

    /// 键数与默认键位：默认表按 QWERTY 序列重排（6 键 = Q..Y）；自定义表保留并补齐（跳过已用过的键）
    #[test]
    /// **按键数记忆键表**：6 改 12 再改回 6，要能找回原来那套键位。
    #[test]
    /// 默认键序是**按键盘物理顺序从第一排到最后一排**。
    #[test]
    fn default_keys_follow_keyboard_order() {
        let first12 = Config::default_kps_keys_for(12);
        assert_eq!(
            first12.first().map(|s| s.as_str()),
            Some("Esc"),
            "第一个应是 Esc，实际 {:?}",
            first12.first()
        );
        assert_eq!(first12.get(1).map(|s| s.as_str()), Some("F1"));
        assert_eq!(first12.get(2).map(|s| s.as_str()), Some("F2"));
        // 前 13 个应是 Esc + F1..F12
        let first13 = Config::default_kps_keys_for(13);
        assert_eq!(first13.len(), 13);
        assert_eq!(first13[12], "F12");
        // 第 14 个开始是主键区第二排
        assert_eq!(Config::default_kps_keys_for(14)[13], "Backquote");
    }

    /// 逗号键本身要能写：逗号是分隔符，所以用 < 或 《 代替。
    #[test]
    fn comma_key_can_be_written_with_angle_or_cjk() {
        let mut cfg = Config::default();
        cfg.kps_keys = "A, <, B".into();
        let names = cfg.kps_key_layout().names();
        assert_eq!(names, vec!["A", ",", "B"], "写 < 应被当成逗号键");

        cfg.kps_keys = "A, 《, B".into();
        let names = cfg.kps_key_layout().names();
        assert_eq!(names, vec!["A", ",", "B"], "写 《 也应被当成逗号键");
    }

    /// 上限应与键盘图里的键数一致。
    #[test]
    /// 默认键序应**跟随配列**：不同配列的起始键都是 Esc、F1…，
    /// 但键数上限与具体键集不同。
    #[test]
    fn default_keys_follow_layout() {
        // 大配列（带 F 区）：Esc, F1, F2 …
        for id in ["104", "98", "87", "75"] {
            let keys = Config::default_kps_keys_for_layout(id, 5);
            assert_eq!(keys.len(), 5, "{id} 配列应能取 5 个键");
            assert_eq!(keys[0], "Esc", "{id} 配列第一个应是 Esc");
            assert_eq!(keys[1], "F1", "{id} 配列第二个应是 F1");
        }
        // 紧凑配列（68/64/60）**没有 F 区**，第二个键是 D1。
        // 这不是 bug —— 这些配列的键盘图上就没有 F1-F12。
        for id in ["68", "64", "60"] {
            let keys = Config::default_kps_keys_for_layout(id, 5);
            assert_eq!(keys.len(), 5, "{id} 配列应能取 5 个键");
            assert_eq!(keys[0], "Esc", "{id} 配列第一个应是 Esc");
            assert_eq!(
                keys[1], "D1",
                "{id} 配列没有 F 区，第二个应是 D1，实际 {:?}",
                keys[1]
            );
        }
        // 87 配列没有小键盘：取全部时不应出现 Num 键
        let all87 = Config::default_kps_keys_for_layout("87", 999);
        assert!(
            !all87.iter().any(|k| k.starts_with("Num")),
            "87 配列不应包含小键盘键：{:?}",
            all87.iter().filter(|k| k.starts_with("Num")).collect::<Vec<_>>()
        );
        // 104 配列应包含小键盘
        let all104 = Config::default_kps_keys_for_layout("104", 999);
        assert!(
            all104.iter().any(|k| k.starts_with("Num")),
            "104 配列应包含小键盘"
        );
    }

    #[test]
    
    /// 迁移后应该能写成多行字符串并读回。
    #[test]
    fn migration_roundtrip() {
        let mut c = Config::default();
        c.layout.keyboard_layout = "104".into();
        // 模拟旧版：单行、顺序与配列默认一致
        c.kps_keys = Config::default_kps_keys_for_layout("104", 104).join(", ");
        assert!(!c.kps_keys.contains('\n'));
        c.migrate_flat_kps_keys();
        assert!(
            c.kps_keys.contains('\n'),
            "迁移后应是多行，实际
{}",
            c.kps_keys
        );
        assert_eq!(c.kps_keys.lines().count(), 6, "应是 6 行");

        // 能写成 TOML 并读回
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(text.contains("\n"), "TOML 里应保留换行");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.kps_keys, c.kps_keys, "读回后键表应一致");
    }

    fn key_max_matches_keyboard_model() {
        assert_eq!(
            KPS_KEY_MAX as usize,
            crate::keys::SYS_END,
            "KPS 监控上限应等于模型里的全部键"
        );
    }

    fn key_count_memory_roundtrip() {
        let mut cfg = Config::default();
        // 起点：6 键的自定义键位
        cfg.kps_key_count = 6;
        cfg.kps_keys = "A,S,D,F,G,H".into();
        // 切到 12 键（应记住 6 键那份）
        cfg.kps_key_count = 12;
        cfg.resize_kps_keys();
        assert_eq!(cfg.kps_key_count, 12);
        assert!(
            cfg.kps_keys_by_count.contains_key("6"),
            "切走时应记住 6 键那份，实际 {:?}",
            cfg.kps_keys_by_count
        );

        // 再切回 6：应恢复 A,S,D,F,G,H
        cfg.kps_key_count = 6;
        cfg.resize_kps_keys();
        assert_eq!(
            cfg.kps_keys, "A,S,D,F,G,H",
            "切回 6 键时应恢复原来那套键位，实际 {}",
            cfg.kps_keys
        );
    }

    /// 键数上限提到 KPS_KEY_MAX，且超出会被夹住。
    #[test]
    fn key_count_allows_more_than_26() {
        assert!(KPS_KEY_MAX >= 60, "上限至少要能装下 6 行 x 10 个");
        let mut cfg = Config::default();
        cfg.kps_key_count = 40;
        cfg.resize_kps_keys();
        let names = cfg.kps_key_layout().names();
        assert!(!names.is_empty(), "40 键应能生成键表");
        assert!(cfg.kps_key_count <= KPS_KEY_MAX);
    }

    fn kps_key_count_rewrites_default_table() {
        let mut cfg = Config::default();
        assert_eq!(cfg.kps_keys, "Q, W, E, R", "默认键位是 QWERTY 前 4 个");
        assert!(cfg.kps_keys_is_default());
        cfg.kps_key_count = 6;
        cfg.resize_kps_keys();
        assert_eq!(cfg.kps_keys, "Q, W, E, R, T, Y", "默认表按新数量重排（6 键 = q..y）");

        cfg.kps_keys = "Z, X, M1, M2".into();
        cfg.kps_key_count = 6;
        cfg.resize_kps_keys();
        assert_eq!(cfg.kps_keys, "Z, X, M1, M2, Q, W", "自定义表补齐时跳过已经用过的键");
        cfg.kps_key_count = 2;
        cfg.resize_kps_keys();
        assert_eq!(cfg.kps_keys, "Z, X", "缩短时只保留前 N 个");
    }
    use super::*;

    #[test]
    fn migrates_legacy_user_dir_once() {
        let base = std::env::temp_dir().join(format!("kmc_cfg_migrate_{}", std::process::id()));
        let (old, new) = (base.join("old"), base.join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("stats.json"), "{\"version\":1}").unwrap();
        std::fs::write(old.join("config.toml"), "language = \"zh\"\n").unwrap();
        std::fs::write(old.join("show.request"), "show").unwrap();

        migrate_legacy(&new, &old);
        assert!(new.join("stats.json").exists(), "stats.json 应搬到程序目录");
        assert!(new.join("config.toml").exists(), "config.toml 应搬到程序目录");
        assert!(!old.join("stats.json").exists(), "老位置不应残留数据文件");
        assert!(!old.join("show.request").exists(), "运行期小文件直接清掉");

        // 程序目录已有数据时不再搬（避免覆盖）
        std::fs::write(old.join("stats.json"), "{\"version\":2}").unwrap();
        migrate_legacy(&new, &old);
        assert_eq!(std::fs::read_to_string(new.join("stats.json")).unwrap(), "{\"version\":1}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn datadir_marker_parsing() {
        let exe = std::env::temp_dir().join(format!("kmc_marker_{}", std::process::id()));
        std::fs::create_dir_all(&exe).unwrap();
        let marker = exe.join("datadir.txt");
        // 注释 + 空行 + 绝对路径（绝对路径的写法随平台不同，Windows 上 "/mnt/..." 不是绝对路径）
        #[cfg(unix)]
        {
            std::fs::write(&marker, "# 数据目录\n\n/mnt/item/KMCounter/\n").unwrap();
            assert_eq!(read_datadir_marker(&exe), Some(PathBuf::from("/mnt/item/KMCounter/")));
        }
        #[cfg(windows)]
        {
            std::fs::write(&marker, "# 数据目录\n\nE:\\项目\\run\\KMCounter\n").unwrap();
            assert_eq!(read_datadir_marker(&exe), Some(PathBuf::from("E:\\项目\\run\\KMCounter")));
        }
        // 相对路径按程序目录解析
        std::fs::write(&marker, "data\n").unwrap();
        assert_eq!(read_datadir_marker(&exe), Some(exe.join("data")));
        // ~ 展开
        std::fs::write(&marker, "~/kmc-data\n").unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(read_datadir_marker(&exe), Some(home.join("kmc-data")));
        // 没有文件 / 只有注释 → None
        std::fs::remove_file(&marker).unwrap();
        assert_eq!(read_datadir_marker(&exe), None);
        std::fs::write(&marker, "# 只有注释\n").unwrap();
        assert_eq!(read_datadir_marker(&exe), None);
        std::fs::remove_dir_all(&exe).ok();
    }

    #[test]
    fn writable_probe_detects_writable_dir() {
        let dir = std::env::temp_dir().join(format!("kmc_cfg_writable_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(dir_writable(&dir));
        assert!(!dir.join(format!(".kmcounter-write-test-{}", std::process::id())).exists(), "探针文件应被删除");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(c.history_days, 365, "默认保留天数应为 365");
        assert!(c.close_to_tray, "默认关闭按钮=隐藏到托盘");
        assert_eq!(c.layout.key_w, 52);
    }

    #[test]
    fn toml_roundtrip() {
        let c = Config::default();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.history_days, 365);
        assert_eq!(back.layout, c.layout);
    }

    /// 配置解析失败：不当成首次运行，且原文件先备份（不被静默吞掉）
    #[test]
    fn broken_config_is_backed_up_not_treated_as_first_run() {
        let dir = std::env::temp_dir().join(format!("kmc_cfg_broken_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "language = \"zh\"\nthis is not toml =\n").unwrap();
        let (cfg, first_run) = Config::load(&path);
        assert!(!first_run, "解析失败不应被当成首次运行");
        assert_eq!(cfg.language, "auto", "退回默认配置");
        // 留档名带时间戳（`config.toml.corrupt-<时间戳>`），不覆盖以前的留档
        let bak = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.file_name().map(|n| n.to_string_lossy().contains(".corrupt-")).unwrap_or(false))
            .expect("原文件应留档为 *.corrupt-*");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "language = \"zh\"\nthis is not toml =\n");
        std::fs::remove_dir_all(dir).ok();
    }

    /// 只读加载不创建文件（命令行 --stats/--export 不该有副作用）
    #[test]
    fn readonly_load_does_not_create_file() {
        let dir = std::env::temp_dir().join(format!("kmc_cfg_readonly_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let cfg = Config::load_readonly(&path);
        assert_eq!(cfg.history_days, 365);
        assert!(!path.exists(), "只读加载不应写出 config.toml");
        std::fs::remove_dir_all(dir).ok();
    }

    /// 悬浮窗位置记忆：配置可来回序列化，None 不写进文件
    #[test]
    /// 新增的两个开关默认值：固定位置**默认关**、始终置顶**默认开**。
    #[test]
    fn hud_lock_and_on_top_defaults() {
        let c = Config::default();
        assert!(!c.hud_lock_pos, "固定位置应默认关");
        assert!(c.hud_always_on_top, "始终置顶应默认开");
    }

    /// 两个开关要能存盘并读回。
    #[test]
    fn hud_lock_and_on_top_roundtrip() {
        let mut c = Config::default();
        c.hud_lock_pos = true;
        c.hud_always_on_top = false;
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(text.contains("hud_lock_pos"), "要写进配置
{text}");
        assert!(text.contains("hud_always_on_top"));
        let back: Config = toml::from_str(&text).unwrap();
        assert!(back.hud_lock_pos);
        assert!(!back.hud_always_on_top);
    }

    fn hud_pos_config_roundtrip() {
        let mut c = Config::default();
        assert_eq!(c.apm_hud_pos, None);
        assert!(c.apm_keep_in_screen && c.kps_keep_in_screen, "屏幕内限制默认开");
        assert_eq!(c.apm_scale, 100);
        c.apm_hud_pos = Some((12.5, 34.0));
        c.kps_hud_pos = Some((-20.0, 8.0));
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(text.contains("apm_hud_pos"), "位置要能写进配置：\n{text}");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.apm_hud_pos, Some((12.5, 34.0)));
        assert_eq!(back.kps_hud_pos, Some((-20.0, 8.0)));
        // None 不落盘（老配置文件不出现空值键）
        let t2 = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(!t2.contains("hud_pos"), "None 不该写进文件：\n{t2}");
    }
}
