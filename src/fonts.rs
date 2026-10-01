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
pub fn install_cjk(defs: &mut FontDefinitions, custom: &str) -> bool {
    let candidates: Vec<std::path::PathBuf> = if !custom.is_empty() {
        vec![std::path::PathBuf::from(custom)]
    } else {
        cjk_candidates()
    };
    for path in candidates {
        if let Ok(data) = std::fs::read(&path) {
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
    }
    false
}
