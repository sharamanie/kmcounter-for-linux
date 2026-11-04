//! KWin **窗口规则**：`~/.config/kwinrulesrc` 的纯文本手术 + 通过 DBus 让 KWin 重载。
//!
//! 规则里维护四条（都带本程序标记，改的时候只动自己的、别人的原样保留）：
//!
//! | 标记 | 作用 |
//! |---|---|
//! | `KMCounter-rs (auto)` | 主窗口：跳过任务栏 + 跳过 Alt-Tab |
//! | `KMCounter-rs HUD (auto)` | HUD：**强制置顶**（Wayland 下 winit 的 always_on_top 是空实现）+ 跳过任务栏/Alt-Tab |
//! | `KMCounter HUD APM (auto)` | APM 悬浮窗的位置记忆（`positionrule=1`：窗口出现时应用一次，之后可自由拖动） |
//! | `KMCounter HUD KPS (auto)` | 同上，KPS |
//!
//! 文本变换全是纯函数（`&str -> String`），副作用只有 `sync_*` 里的「写文件 + 通知重载」，
//! 且**内容没变就不写不重载**（幂等）—— 所以能用快照测试把行为钉住。
use super::{hud_pos_supported, is_kde, rules_path};

const MARKER: &str = "KMCounter-rs (auto)";
/// HUD 悬浮窗规则（置顶）单独一个标记，便于与主窗口规则分开维护
const HUD_MARKER: &str = "KMCounter-rs HUD (auto)";
const HUD_POS_APM_MARKER: &str = "KMCounter HUD APM (auto)";
const HUD_POS_KPS_MARKER: &str = "KMCounter HUD KPS (auto)";
const WM_CLASS: &str = "kmcounter";

/// 主窗口规则体：跳过任务栏 + 跳过 Alt-Tab
fn main_rule_body() -> String {
    format!(
        "skiptaskbar=true\nskiptaskbarrule=2\nskipswitcher=true\nskipswitcherrule=2\nwmclass={WM_CLASS}\nwmclassmatch=1\nwmclasscomplete=false\ntypes=1\n"
    )
}

/// HUD 悬浮窗规则体：**强制置顶** + 跳过任务栏/Alt-Tab。
///
/// 为什么必须靠 KWin 规则置顶：winit 的 Wayland 后端 `set_window_level` 是**空实现**
/// （`ViewportBuilder::with_always_on_top()` 在 Wayland 下完全无效），所以 HUD 的"置顶"
/// 只能由合成器侧的 KWin 规则（`above=true` 强制）来保证。
/// 用标题子串（`titlematch=2`）把 HUD（"KMC - APM" / "KMC - KPS"）与主窗口（"KMCounter-rs"）区分开。
fn hud_rule_body() -> String {
    format!(
        "above=true\naboverule=2\nskiptaskbar=true\nskiptaskbarrule=2\nskipswitcher=true\nskipswitcherrule=2\nwmclass={WM_CLASS}\nwmclassmatch=1\nwmclasscomplete=false\ntitle=KMC -\ntitlematch=2\ntypes=1\n"
    )
}

/// 启用/停用「跳过任务栏 + 跳过 Alt-Tab」规则（幂等）。
///
/// 同时维护两条规则：主窗口一条、HUD 悬浮窗一条（后者额外 `above=true` 强制置顶，
/// 因为 Wayland 下 winit 的 always_on_top 是空实现）。关掉时两条一起移除。
pub fn set_skip_taskbar(enabled: bool) {
    if !is_kde() {
        if enabled {
            log::info!("非 KDE 会话，跳过任务栏规则不适用（Windows/X11 可直接隐藏窗口）");
        }
        return;
    }
    let path = rules_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let has_main = text.contains(MARKER);
    let has_hud = text.contains(HUD_MARKER);
    if enabled == (has_main && has_hud) {
        return; // 已是目标状态
    }
    let new_text = if enabled {
        // 逐条补齐：从旧版本升级过来时可能只有主窗口那条（HUD 置顶则缺失）
        let mut t = if has_main { text.clone() } else { add_rule(&text) };
        if !has_hud {
            t = add_hud_rule(&t);
        }
        t
    } else {
        remove_rule(&text)
    };
    if new_text == text {
        log::warn!("KWin 规则未发生变化，跳过写入");
        return;
    }
    // 首次修改前备份
    let bak = path.with_extension("bak");
    if path.exists() && !bak.exists() {
        let _ = std::fs::copy(&path, &bak);
    }
    if let Err(e) = std::fs::write(&path, &new_text) {
        log::error!("写入 KWin 规则失败: {e}");
        return;
    }
    match reconfigure() {
        Ok(_) => log::info!(
            "已{} KWin 规则（跳过任务栏/Alt-Tab{}）",
            if enabled { "启用" } else { "移除" },
            if enabled { " + HUD 置顶" } else { "" }
        ),
        Err(e) => log::warn!("已写入规则但通知 KWin 重载失败（重启或重新登录后生效）: {e}"),
    }
}

/// 让 KWin 重新加载配置
fn reconfigure() -> Result<(), String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
    conn.call_method(Some("org.kde.KWin"), "/KWin", Some("org.kde.KWin"), "reconfigure", &())
        .map_err(|e| e.to_string())?;
    Ok(())
}


fn kv_get(kv: &[(String, String)], key: &str) -> Option<String> {
    kv.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn kv_set(kv: &mut Vec<(String, String)>, key: &str, val: String) {
    if let Some((_, v)) = kv.iter_mut().find(|(k, _)| k == key) {
        *v = val;
    } else {
        kv.push((key.to_string(), val));
    }
}

/// 在 INI 文本中追加本程序规则（若不存在）：主窗口（跳过任务栏 / Alt-Tab）
fn add_rule(text: &str) -> String {
    add_rule_with(text, MARKER, &main_rule_body())
}

/// 在 INI 文本中追加 HUD 悬浮窗规则（置顶 + 跳过任务栏 / Alt-Tab）
fn add_hud_rule(text: &str) -> String {
    add_rule_with(text, HUD_MARKER, &hud_rule_body())
}

/// 在 INI 文本中追加一条带标记的规则（自动分配新 id 并登记到 [General] rules）
fn add_rule_with(text: &str, marker: &str, body: &str) -> String {
    let (mut general, groups) = split_ini(text);
    let next_id = groups
        .iter()
        .filter_map(|(name, _)| name.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let id = next_id.to_string();
    let mut rules: Vec<String> = kv_get(&general, "rules")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    if !rules.iter().any(|r| r == &id) {
        rules.push(id.clone());
    }
    kv_set(&mut general, "count", rules.len().to_string());
    kv_set(&mut general, "rules", rules.join(","));

    let mut out = render_general(&general);
    for (name, group_body) in &groups {
        out.push_str(&format!("\n[{name}]\n{group_body}"));
    }
    out.push_str(&format!("\n[{id}]\nDescription={marker}\n{body}"));
    out
}

/// 从 INI 文本中删除本程序的两条规则（主窗口 + HUD，若存在），保留其他规则
fn remove_rule(text: &str) -> String {
    let (mut general, groups) = split_ini(text);
    let kept: Vec<(String, String)> = groups
        .iter()
        .filter(|(_, body)| !body.contains(MARKER) && !body.contains(HUD_MARKER))
        .cloned()
        .collect();
    if kept.len() == groups.len() {
        return text.to_string();
    }
    let ids: Vec<String> = kept.iter().map(|(n, _)| n.clone()).collect();
    let filtered: Vec<String> = kv_get(&general, "rules")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| ids.contains(s)).collect())
        .unwrap_or_default();
    kv_set(&mut general, "count", filtered.len().to_string());
    kv_set(&mut general, "rules", filtered.join(","));

    let mut out = render_general(&general);
    for (name, body) in &kept {
        out.push_str(&format!("\n[{name}]\n{body}"));
    }
    out
}

/// 解析 INI：返回 ([General] 键值对, 其余分组(名, 原文体))
fn split_ini(text: &str) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let mut general: Vec<(String, String)> = Vec::new();
    let mut groups: Vec<(String, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None; // (name, body)
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if let Some((name, body)) = cur.take() {
                if name == "General" {
                    for l in body.lines() {
                        if let Some((k, v)) = l.split_once('=') {
                            general.push((k.trim().to_string(), v.trim().to_string()));
                        }
                    }
                } else {
                    groups.push((name, body));
                }
            }
            cur = Some((t[1..t.len() - 1].to_string(), String::new()));
        } else if let Some((_, body)) = cur.as_mut() {
            if !t.is_empty() {
                body.push_str(t);
                body.push('\n');
            }
        }
    }
    if let Some((name, body)) = cur.take() {
        if name == "General" {
            for l in body.lines() {
                if let Some((k, v)) = l.split_once('=') {
                    general.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
        } else {
            groups.push((name, body));
        }
    }
    (general, groups)
}

/// 渲染 [General] 段（count/rules 在前，其余保留）
fn render_general(kv: &[(String, String)]) -> String {
    let mut out = String::from("[General]\n");
    for key in ["count", "rules"] {
        if let Some((_, v)) = kv.iter().find(|(k, _)| k == key) {
            out.push_str(&format!("{key}={v}\n"));
        }
    }
    for (k, v) in kv.iter().filter(|(k, _)| k != "count" && k != "rules") {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}


fn hud_pos_rule_body(pos: (f32, f32), title: &str) -> String {
    format!(
        "position={},{}\npositionrule=1\ntitle={title}\ntitlematch=1\ntypes=1\nwmclass=kmcounter\nwmclasscomplete=false\nwmclassmatch=1",
        pos.0, pos.1
    )
}

/// 就地更新某条 HUD 规则的 position（节存在时）：避免 remove+add 让规则节号无限膨胀。
/// 返回 None 表示没找到该规则（调用方走 add）。
fn update_rule_position(text: &str, marker: &str, pos: (f32, f32)) -> Option<String> {
    if !text.contains(marker) {
        return None;
    }
    let new_line = format!("position={},{}", pos.0, pos.1);
    let mut out = String::with_capacity(text.len() + 16);
    let mut in_target = false;
    let mut replaced = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            in_target = false;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.starts_with("Description=") && t.contains(marker) {
            in_target = true;
        }
        // "position=" 不会误匹配 "positionrule="（第 9 个字符是 'r' 不是 '='）
        if in_target && !replaced && t.starts_with("position=") {
            if t != new_line {
                out.push_str(&new_line);
                out.push('\n');
            } else {
                out.push_str(line);
                out.push('\n');
            }
            replaced = true;
            in_target = false;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // lines() 会丢掉文件末尾的换行 —— 保持原样，保证幂等（重复同步文本逐字节一致）
    if text.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    let _ = replaced;
    Some(out)
}

/// 从 INI 文本中删除指定 marker 的规则节（保留其他规则），General 的 count/rules 同步
fn remove_rule_by_marker(text: &str, marker: &str) -> String {
    let (mut general, groups) = split_ini(text);
    let kept: Vec<(String, String)> = groups
        .iter()
        .filter(|(_, body)| !body.contains(marker))
        .cloned()
        .collect();
    if kept.len() == groups.len() {
        return text.to_string();
    }
    let ids: Vec<String> = kept.iter().map(|(n, _)| n.clone()).collect();
    let filtered: Vec<String> = kv_get(&general, "rules")
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| ids.contains(&s.to_string()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    kv_set(&mut general, "count", filtered.len().to_string());
    kv_set(&mut general, "rules", filtered.join(","));
    let mut out = render_general(&general);
    for (name, body) in &kept {
        out.push_str(&format!("\n[{name}]\n{body}"));
    }
    out
}

/// 纯函数：在 kwinrulesrc 文本中同步两条 HUD 位置规则（有位置 = upsert，无 = 移除）
fn apply_hud_rules(text: &str, apm: Option<(f32, f32)>, kps: Option<(f32, f32)>) -> String {
    let mut t = text.to_string();
    for (marker, title, pos) in [
        (HUD_POS_APM_MARKER, "KMC - APM", apm),
        (HUD_POS_KPS_MARKER, "KMC - KPS", kps),
    ] {
        match pos {
            Some(p) => {
                if let Some(updated) = update_rule_position(&t, marker, p) {
                    t = updated;
                } else {
                    t = add_rule_with(&t, marker, &hud_pos_rule_body(p, title));
                }
            }
            None => t = remove_rule_by_marker(&t, marker),
        }
    }
    t
}

/// 同步 HUD 位置的 **KWin 窗口规则**（恢复位置的正道）：规则由 KWin 核心在窗口 map 时
/// 应用位置 —— 不经过摆放流程竞争，跨屏也没问题（脚本赋值会被初始摆放覆盖，实测）。
/// 幂等：文本没变就不写不重载。
pub fn sync_hud_rules(apm_pos: Option<(f32, f32)>, kps_pos: Option<(f32, f32)>) {
    if !hud_pos_supported() {
        return;
    }
    let path = rules_path();
    let orig = std::fs::read_to_string(&path).unwrap_or_default();
    let new_text = apply_hud_rules(&orig, apm_pos, kps_pos);
    if new_text == orig {
        return;
    }
    let bak = path.with_extension("bak-hudpos");
    if path.exists() && !bak.exists() {
        let _ = std::fs::copy(&path, &bak);
    }
    if let Err(e) = std::fs::write(&path, &new_text) {
        log::error!("写入 HUD 位置规则失败: {e}");
        return;
    }
    match reconfigure() {
        Ok(_) => log::info!("HUD 位置规则已同步（窗口出现时由 KWin 恢复到记忆位置）"),
        Err(e) => log::warn!("HUD 位置规则已写入但 KWin 重载失败: {e}"),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[General]\ncount=2\nrules=1,7\n\n[1]\nDescription=Konsole 置顶\nabove=true\naboverule=2\nwmclass=konsole\n\n[7]\nDescription=别的规则\nopacityactive=90\n";

    #[test]
    fn add_keeps_others() {
        let out = add_rule(SAMPLE);
        assert!(out.contains(MARKER));
        assert!(out.contains("Description=Konsole 置顶"));
        assert!(out.contains("Description=别的规则"));
        assert!(out.contains("rules=1,7,8"), "rules 列表应追加新 id: {out}");
        assert!(out.contains("count=3"));
        assert!(out.contains("wmclass=kmcounter"));
        assert!(out.contains("skiptaskbarrule=2"));
        assert!(out.contains("skipswitcherrule=2"));
    }

    #[test]
    fn add_then_remove_restores() {
        let added = add_rule(SAMPLE);
        let removed = remove_rule(&added);
        assert!(!removed.contains(MARKER));
        assert!(removed.contains("Description=Konsole 置顶"));
        assert!(removed.contains("Description=别的规则"));
        assert!(removed.contains("rules=1,7"));
        assert!(removed.contains("count=2"));
        assert!(!removed.contains("[8]"));
    }

    #[test]
    fn remove_noop_without_rule() {
        assert_eq!(remove_rule(SAMPLE), SAMPLE);
    }

    #[test]
    fn empty_file_supported() {
        let out = add_rule("");
        assert!(out.contains("[General]"));
        assert!(out.contains("rules=1"));
        assert!(out.contains(MARKER));
    }

    /// HUD 规则：强制置顶（Wayland 下 winit 的 always_on_top 是空实现）+ 标题子串匹配；
    /// 主窗口规则不能被带上置顶；删除时两条规则一起走、别人的规则保留
    #[test]
    fn hud_rule_forces_above_and_is_removed_together() {
        let with_main = add_rule(SAMPLE);
        let both = add_hud_rule(&with_main);
        assert!(both.contains(HUD_MARKER), "应有 HUD 规则标记");
        assert!(both.contains("above=true"), "HUD 规则应置顶");
        assert!(both.contains("aboverule=2"), "置顶必须是强制规则（rule=2）");
        assert!(both.contains("title=KMC -"), "用标题子串把 HUD 与主窗口区分开");
        assert!(both.contains("titlematch=2"), "标题按子串匹配，覆盖 APM/KPS 两个 HUD");
        assert!(both.contains("rules=1,7,8,9"), "两条规则都要登记：{both}");

        let main_section = both.split("[8]").nth(1).unwrap().split("[9]").next().unwrap();
        assert!(main_section.contains("skiptaskbar=true"));
        assert!(!main_section.contains("above=true"), "主窗口规则不该强制置顶：{main_section}");

        let removed = remove_rule(&both);
        assert!(!removed.contains(MARKER) && !removed.contains(HUD_MARKER), "两条规则一起删除");
        assert!(removed.contains("Description=Konsole 置顶"), "别人的规则保留");
        assert!(removed.contains("rules=1,7"));
    }

    /// 手动运行：把两条 KWin 规则（主窗口跳过任务栏 + HUD 强制置顶）写进**真实**配置并让 KWin 重载，
    /// 同时打印最终文件内容，便于在真机上核对规则是否被 KWin 接受。仅在 KDE 会话下有效。
    ///
    /// `XDG_CURRENT_DESKTOP=KDE cargo test --release -- --ignored write_kde_rules_now --nocapture`
    #[test]
    #[ignore = "会写入真实的 ~/.config/kwinrulesrc"]
    fn write_kde_rules_now() {
        set_skip_taskbar(true);
        let text = std::fs::read_to_string(rules_path()).unwrap_or_default();
        println!("---- {}\n{text}", rules_path().display());
        assert!(text.contains(MARKER), "应写入主窗口规则");
        assert!(text.contains(HUD_MARKER), "应写入 HUD 置顶规则");
        assert!(text.contains("above=true"), "HUD 规则必须置顶");
    }

    /// HUD 位置规则的同步：upsert / 就地更新 / 移除 / 幂等（纯文本变换）
    #[test]
    fn hud_rules_sync_upserts_updates_and_removes() {
        let base = "[General]\ncount=1\nrules=1\n\n[1]\nDescription=other\nfoo=bar\n";
        // 新增两条
        let t = apply_hud_rules(base, Some((10.0, 20.0)), Some((30.0, 40.0)));
        assert!(t.contains("Description=KMCounter HUD APM (auto)"));
        assert!(t.contains("position=10,20"));
        assert!(t.contains("positionrule=1"), "Apply 语义（窗口出现时应用一次，可拖动）");
        assert!(t.contains("title=KMC - APM") && t.contains("titlematch=1"));
        assert!(t.contains("Description=KMCounter HUD KPS (auto)") && t.contains("position=30,40"));
        assert!(t.contains("rules=1,2,3"), "General 的 rules 列表要跟上");
        // 幂等：首次同步会把 add 生成文本的文件尾规范成带换行（良性，一次性），
        // 之后必须逐字节稳定（不膨胀）
        let t2 = apply_hud_rules(&t, Some((10.0, 20.0)), Some((30.0, 40.0)));
        assert_eq!(t2.trim_end(), t.trim_end(), "重复同步内容应无变化");
        let t2b = apply_hud_rules(&t2, Some((10.0, 20.0)), Some((30.0, 40.0)));
        assert_eq!(t2b, t2, "稳态后应逐字节幂等");
        // 就地更新位置
        let t3 = apply_hud_rules(&t, Some((10.0, 20.0)), Some((50.0, 60.0)));
        assert!(t3.contains("position=50,60"));
        assert!(t3.contains("position=10,20"), "APM 的规则不受影响");
        assert!(!t3.contains("position=30,40"), "旧位置应被更新掉");
        // positionrule 不能被误伤（starts_with("position=") 不匹配 "positionrule="）
        assert!(t3.contains("positionrule=1"));
        // 移除全部：无关规则保留
        let t4 = apply_hud_rules(&t3, None, None);
        assert!(!t4.contains("KMCounter HUD"));
        assert!(t4.contains("Description=other"));
        assert!(t4.contains("rules=1"));
    }
}
