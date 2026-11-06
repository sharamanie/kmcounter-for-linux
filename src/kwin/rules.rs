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
/// HUD **设置窗**规则（与 HUD 本体分开）
const SETTINGS_MARKER: &str = "KMCounter HUD settings (auto)";
const HUD_POS_APM_MARKER: &str = "KMCounter HUD APM (auto)";
const HUD_POS_KPS_MARKER: &str = "KMCounter HUD KPS (auto)";
const WM_CLASS: &str = "kmcounter";

/// 主窗口规则体：跳过任务栏 + 跳过 Alt-Tab
fn main_rule_body() -> String {
    format!(
        "skiptaskbar=true\nskiptaskbarrule=2\nskipswitcher=true\nskipswitcherrule=2\nwmclass={WM_CLASS}\nwmclassmatch=1\nwmclasscomplete=false\ntypes=1\n"
    )
}

/// HUD 设置窗规则体：**强制置顶** + 跳过任务栏/Alt-Tab。
///
/// 为什么要单独一条：
/// HUD 自己的规则用 `title=KMC -` 子串匹配，设置窗如果也叫 "KMC - ..."，
/// 就会被同一条规则拿下 —— 两个窗口在同一个 above 层上，
/// 固定 HUD 后设置窗会被压在下面、点不到。现在设置窗用 "KMC# " 前缀，
/// 匹配条件也独立，两者互不干扰。
fn settings_rule_body() -> String {
    format!(
        "above=true\naboverule=2\nskiptaskbar=true\nskiptaskbarrule=2\nskipswitcher=true\nskipswitcherrule=2\nwmclass={WM_CLASS}\nwmclassmatch=1\nwmclasscomplete=false\ntitle=KMC# \ntitlematch=2\ntypes=1\n"
    )
}

/// 在 INI 文本中追加设置窗规则（若不存在）
fn add_settings_rule(text: &str) -> String {
    add_rule_with(text, SETTINGS_MARKER, &settings_rule_body())
}

/// HUD 悬浮窗规则体：**强制置顶** + 跳过任务栏/Alt-Tab。
///
/// 为什么必须靠 KWin 规则置顶：winit 的 Wayland 后端 `set_window_level` 是**空实现**
/// （`ViewportBuilder::with_always_on_top()` 在 Wayland 下完全无效），所以 HUD 的"置顶"
/// 只能由合成器侧的 KWin 规则（`above=true` 强制）来保证。
/// 用标题子串（`titlematch=2`）把 HUD（"KMC - APM" / "KMC - KPS"）与主窗口（"KMCounter-rs"）区分开。
fn hud_rule_body(above: bool) -> String {
    // above=true + aboverule=2 = **强制**保持在所有窗口之上（rule=2 是"强制"）。
    // 关掉 「始终置顶」时这两行必须整条去掉，否则 KWin 会一直把 HUD 压在最上层，
    // winit 侧 with_always_on_top(false) 完全无效（Wayland 下该调用本来就是空实现）。
    let above_part = if above { "above=true\naboverule=2\n" } else { "" };
    format!(
        "{above_part}skiptaskbar=true\nskiptaskbarrule=2\nskipswitcher=true\nskipswitcherrule=2\nwmclass={WM_CLASS}\nwmclassmatch=1\nwmclasscomplete=false\ntitle=KMC -\ntitlematch=2\ntypes=1\n"
    )
}

static LAST_APPLIED_ABOVE: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

/// 运行时切换 HUD 始终置顶（设置面板里改开关时调用，立即生效；幂等，值未变时不重复下发 D-Bus 脚本）。
///
/// 只动已有 HUD 规则的 above 两行，不新建规则（那是 set_skip_taskbar 的职责）。
pub fn set_hud_above_public(above: bool) {
    if !is_kde() {
        return;
    }
    let path = rules_path();
    let mut rule_changed = false;
    if let Ok(text) = std::fs::read_to_string(&path) {
        let new_text = set_hud_above(&text, above);
        if new_text != text {
            rule_changed = true;
            if let Err(e) = std::fs::write(&path, &new_text) {
                log::error!("写入 HUD 置顶规则失败: {e}");
            } else {
                match reconfigure() {
                    Ok(_) => log::info!("HUD 置顶规则已同步（always_on_top={above}）"),
                    Err(e) => log::warn!("HUD 置顶规则已写入但 KWin 重载失败: {e}"),
                }
            }
        }
    }
    let mut last = LAST_APPLIED_ABOVE.lock().unwrap_or_else(|e| e.into_inner());
    if !rule_changed && *last == Some(above) {
        return;
    }
    *last = Some(above);
    drop(last);
    // 无论是新规则还是已有规则，立即向 KWin 运行中的悬浮窗下发 keepAbove 属性
    super::script::apply_hud_above(above);
    log::info!("HUD 置顶属性已下发（keepAbove={above}）");
}

/// 启用/停用「跳过任务栏 + 跳过 Alt-Tab」规则（幂等）。
///
/// 同时维护两条规则：主窗口一条、HUD 悬浮窗一条（后者额外 `above=true` 强制置顶，
/// 因为 Wayland 下 winit 的 always_on_top 是空实现）。关掉时两条一起移除。
pub fn set_skip_taskbar(enabled: bool, hud_above: bool) {
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
    // 开关状态已对上，**且** HUD 规则里的 above 也已经是期望值时才能提前返回。
    // 必须在 HUD 规则自身的分组体内检查，避免被 SETTINGS_MARKER 的 aboverule 干扰。
    let (_, groups) = split_ini(&text);
    let hud_group = groups.iter().find(|(_, body)| body.contains(HUD_MARKER));
    let above_ok = match hud_group {
        Some((_, body)) => {
            if hud_above {
                body.contains("above=true") && body.contains("aboverule=2")
            } else {
                !body.contains("above=true") && !body.contains("aboverule=2")
            }
        }
        None => false,
    };
    let has_settings = text.contains(SETTINGS_MARKER);
    if enabled == (has_main && has_hud) && above_ok && (has_settings || !enabled) {
        return; // 已是目标状态
    }
    let new_text = if enabled {
        // 逐条补齐：从旧版本升级过来时可能只有主窗口那条（HUD 置顶则缺失）
        let mut t = if has_main { text.clone() } else { add_rule(&text) };
        if !has_hud {
            t = add_hud_rule(&t, hud_above);
        }
        // 设置窗规则（独立前缀 "KMC# "）：不建它的话，固定 HUD 后
        // 设置窗会被 HUD 盖住、点不到。
        if !t.contains(SETTINGS_MARKER) {
            t = add_settings_rule(&t);
        }
        // 已存在的 HUD 规则要把 above 同步过去（开关改了也要能生效）
        t = set_hud_above(&t, hud_above);
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
fn add_hud_rule(text: &str, above: bool) -> String {
    add_rule_with(text, HUD_MARKER, &hud_rule_body(above))
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

/// 在已有的 HUD 规则里**开/关置顶**（原地增或删 `above=true` + `aboverule=2` 两行）。
///
/// 为什么不能只改 winit：Wayland 下 `with_always_on_top()` 是**空实现**，
/// HUD 的置顶完全靠 KWin 规则。不把这两行拿掉，关掉开关也不会有任何变化。
fn set_hud_above(text: &str, above: bool) -> String {
    if !text.contains(HUD_MARKER) {
        return text.to_string();
    }
    // 用与其他规则函数相同的 INI 解析 / 重建路径。
    //
    // 曾经用逐行拼接，结果把 `[General]` 段的 count/rules 也弄丢了 ——
    // KWin 发现 rules 列表与实际段不一致就会丢弃其他规则（实测：
    // 主窗口与 APM 两条规则被整条吞掉）。
    let (mut general, groups) = split_ini(text);
    let mut out = render_general(&general);
    for (name, body) in &groups {
        if !body.contains(HUD_MARKER) {
            out.push_str(&format!("\n[{name}]\n{body}"));
            continue;
        }
        // 重建这一段：过滤掉旧的 above 两行，需要时在开头补回
        let mut kw: Vec<(String, String)> = Vec::new();
        for l in body.lines() {
            let t = l.trim();
            if t.is_empty() {
                continue;
            }
            if t.starts_with("above=") || t.starts_with("aboverule=") {
                continue;
            }
            if let Some((k, v)) = t.split_once('=') {
                kw.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        if above {
            kw.insert(0, ("aboverule".into(), "2".into()));
            kw.insert(0, ("above".into(), "true".into()));
        }
        let mut body2 = String::new();
        for (k, v) in &kw {
            body2.push_str(&format!("{k}={v}\n"));
        }
        out.push_str(&format!("\n[{name}]\n{body2}"));
    }
    let _ = &mut general;
    out
}

/// 从 INI 文本中删除本程序的两条规则（主窗口 + HUD，若存在），保留其他规则
fn remove_rule(text: &str) -> String {
    let (mut general, groups) = split_ini(text);
    let kept: Vec<(String, String)> = groups
        .iter()
        .filter(|(_, body)| {
            !body.contains(MARKER) && !body.contains(HUD_MARKER) && !body.contains(SETTINGS_MARKER)
        })
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

    /// **设置窗规则必须存在且与 HUD 规则分开。**
    ///
    /// 用户反馈：固定 HUD 后，HUD 设置窗被压在 HUD 下面、点不到。
    /// 根因：设置窗原来也叫 "KMC - …"，被 HUD 的 title=KMC - 规则一并拿下，
    /// 两个窗口同层，谁在上面取决于创建顺序。
    #[test]
    fn settings_rule_is_separate_from_hud() {
        let mut text = add_rule("");
        text = add_hud_rule(&text, true);
        text = add_settings_rule(&text);

        // 设置窗规则自己存在、强制置顶、用独立前缀
        let sec = text
            .split("[")
            .find(|s| s.contains(SETTINGS_MARKER))
            .expect("应有设置窗规则");
        assert!(sec.contains("above=true"), "设置窗应强制置顶
{sec}");
        assert!(sec.contains("aboverule=2"));
        assert!(
            sec.contains("title=KMC# "),
            "设置窗规则应匹配 KMC# 前缀（与 HUD 的 KMC - 分开）
{sec}"
        );

        // HUD 规则的匹配条件不应该能匹配到设置窗
        let hud_sec = text
            .split("[")
            .find(|s| s.contains(HUD_MARKER))
            .expect("应有 HUD 规则");
        assert!(
            !hud_sec.contains("KMC#"),
            "HUD 规则不应匹配 KMC# 前缀
{hud_sec}"
        );
    }

    /// 关掉跳过任务栏时，三条规则要一起清掉。
    #[test]
    fn settings_rule_removed_together() {
        let mut text = add_rule("");
        text = add_hud_rule(&text, true);
        text = add_settings_rule(&text);
        assert!(text.contains(SETTINGS_MARKER));
        let off = remove_rule(&text);
        assert!(
            !off.contains(SETTINGS_MARKER),
            "关掉后设置窗规则也要清掉
{off}"
        );
    }

    /// **关掉「始终置顶」时，规则里的 above 两行必须真的没了。
    ///
    /// 用户反馈：取消置顶后 HUD 仍然一直在最上层。
    /// 原因：Wayland 下 winit 的 with_always_on_top() 是空实现，置顶完全靠 KWin 规则
    /// 里写死的 above=true；光改 winit 层没用。
    #[test]
    fn hud_above_toggles_in_rule() {
        let with_main = add_rule("");
        // 默认开：规则里有 above
        let on = add_hud_rule(&with_main, true);
        assert!(on.contains("above=true"), "开启时应置顶：{on}");
        assert!(on.contains("aboverule=2"), "必须是强制置顶");

        // 关掉：above 两行应被删掉，但跳过任务栏等规则要保留
        let off = set_hud_above(&on, false);
        assert!(!off.contains("above=true"), "关掉后不该再有 above=true：{off}");
        assert!(!off.contains("aboverule="), "关掉后不该再有 aboverule：{off}");
        assert!(off.contains("skiptaskbar=true"), "跳过任务栏应保留");
        assert!(off.contains(HUD_MARKER), "HUD 规则本身应保留");
        assert!(off.contains("title=KMC -"), "匹配条件应保留");

        // 再开回来：above 重新出现（幂等）
        let back_on = set_hud_above(&off, true);
        assert!(back_on.contains("above=true"), "重新开启应恢复置顶：{back_on}");
        assert!(back_on.contains("aboverule=2"));
        // 幂等：再调一次应不再变
        assert_eq!(set_hud_above(&back_on, true), back_on);
    }

    /// **修复回归**：改 above 不能把 `[General]` 或其他规则弄丢。
    ///
    /// 曾经用逐行拼接重建文件，结果 `count`/`rules` 丢失，
    /// KWin 随即丢弃了主窗口与 APM 两条规则（实测真实发生过）。
    #[test]
    fn set_hud_above_preserves_general_and_other_rules() {
        // 构造：主窗口规则 + HUD 规则 + 一条别人的规则
        let mut text = add_rule("");
        text = add_hud_rule(&text, true);
        text = add_rule_with(
            &text,
            "别人的规则",
            "wmclass=konsole\nopacityactive=90\n",
        );
        assert!(text.contains("count=3"), "前提：应有 3 条规则\n{text}");

        let off = set_hud_above(&text, false);
        assert!(off.contains("[General]"), "必须保留 General 段\n{off}");
        assert!(off.contains("count=3"), "必须保留 count\n{off}");
        assert!(off.contains("rules="), "必须保留 rules 列表\n{off}");
        assert!(!off.contains("above=true"), "above 应已移除\n{off}");
        // 主窗口规则与别人的规则都还在
        assert!(off.contains(MARKER), "主窗口规则丢了\n{off}");
        assert!(off.contains(HUD_MARKER), "HUD 规则丢了\n{off}");
        assert!(off.contains("别人的规则"), "他人规则丢了\n{off}");
        assert!(off.contains("opacityactive=90"), "他人规则内容丢了\n{off}");
        assert!(off.contains("wmclass=konsole"), "他人规则内容丢了\n{off}");

        // 再开回来，依然完整
        let on = set_hud_above(&off, true);
        assert!(on.contains("above=true"));
        assert!(on.contains("count=3"), "重开后 count 也要在\n{on}");
        assert!(on.contains("别人的规则"));
    }

    /// 没有 HUD 规则时不该凭空造一条。
    #[test]
    fn set_hud_above_is_noop_without_rule() {
        let text = "[General]\ncount=0\nrules=\n";
        assert_eq!(set_hud_above(text, true), text);
        assert_eq!(set_hud_above(text, false), text);
    }

    /// HUD 规则：强制置顶（Wayland 下 winit 的 always_on_top 是空实现）+ 标题子串匹配；
    /// 主窗口规则不能被带上置顶；删除时两条规则一起走、别人的规则保留
    #[test]
    fn hud_rule_forces_above_and_is_removed_together() {
        let with_main = add_rule(SAMPLE);
        let both = add_hud_rule(&with_main, true);
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
        set_skip_taskbar(true, true);
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
