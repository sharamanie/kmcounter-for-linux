//! 字体：为 egui 装载中文字体（默认字体不含 CJK）。

use egui::FontDefinitions;

/// 常见中文字体候选路径（顺序即优先级）
fn cjk_candidates() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        for f in ["msyh.ttc", "msyh.ttf", "msyhbd.ttc", "simhei.ttf", "simsun.ttc"] {
            v.push(std::path::PathBuf::from(&windir).join("Fonts").join(f));
        }
    } else {
        for f in [
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJKsc-Regular.ttc",
            "/usr/share/fonts/wenquanyi-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/wenquanyi-zenhei/wqy-zenhei.ttc",
            "/usr/share/fonts/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        ] {
            v.push(std::path::PathBuf::from(f));
        }
    }
    v
}

/// 尝试装载中文字体。返回是否成功。
/// 自定义路径失败时回退系统候选字体（路径写错一个字符不该让中文界面降级成英文）。
pub fn install_cjk(defs: &mut FontDefinitions, custom: &str) -> bool {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    let has_custom = !custom.is_empty();
    if has_custom {
        candidates.push(std::path::PathBuf::from(custom));
    }
    candidates.extend(cjk_candidates());
    for (i, path) in candidates.iter().enumerate() {
        match std::fs::read(path) {
            Ok(data) => {
                defs.font_data.insert(
                    "cjk".to_owned(),
                    egui::FontData::from_owned(data).into(),
                );
                if let Some(prop) = defs.families.get_mut(&egui::FontFamily::Proportional) {
                    prop.push("cjk".to_owned());
                }
                if let Some(mono) = defs.families.get_mut(&egui::FontFamily::Monospace) {
                    mono.push("cjk".to_owned());
                }
                log::info!("已加载中文字体: {}", path.display());
                return true;
            }
            Err(e) => {
                if has_custom && i == 0 {
                    log::warn!("自定义中文字体 {} 加载失败({e})，回退系统候选字体", path.display());
                }
            }
        }
    }
    false
}
