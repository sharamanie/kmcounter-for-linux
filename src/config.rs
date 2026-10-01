//! TOML 配置文件。默认值与原版 KMCounter 一致（存储 30 天、键宽 52/键高 45/键间距 2/区域间距 10）。

use serde::{Deserialize, Serialize};
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
    /// 区域垂直间距（像素）
    pub v_spacing: u32,
}

impl Default for LayoutCfg {
    fn default() -> Self {
        LayoutCfg { key_w: 52, key_h: 45, key_spacing: 2, h_spacing: 10, v_spacing: 10 }
    }
}

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
            limit_move_km: 20.0,
            limit_clicks: 0,
            limit_keystrokes: 0,
            welcome_shown: false,
            layout: LayoutCfg::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> (Config, bool) {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(c) => (c, false),
                Err(e) => {
                    log::warn!("配置解析失败({e})，使用默认配置");
                    (Config::default(), true)
                }
            },
            Err(_) => {
                let c = Config::default();
                let _ = c.save(path);
                (c, true) // 第二个返回值：是否为首次运行
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
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
";
        std::fs::write(path, format!("{header}{body}"))
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
}
