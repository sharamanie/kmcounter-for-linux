//! KDE (KWin) 集成：Wayland 下 winit 无法隐藏窗口、也没有“跳过任务栏”协议，
//! 因此通过 KWin 窗口规则实现“只在托盘、不占任务栏/Alt-Tab”，
//! 并用 1x1 收缩代替隐藏（winit 在 Wayland 下 set_visible 是空实现）。
//!
//! 规则写入 `~/.config/kwinrulesrc`（只增删带标记的本程序规则，保留用户其他规则），
//! 改完通过 DBus 通知 KWin 重新加载。写文件前会备份为 kwinrulesrc.bak。

#![cfg(target_os = "linux")]

use std::path::PathBuf;

const MARKER: &str = "KMCounter-rs (auto)";
const WM_CLASS: &str = "kmcounter";

/// 是否 KDE 会话（只有 KWin 支持这套窗口规则）
pub fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_uppercase().contains("KDE"))
        .unwrap_or(false)
}

fn rules_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("kwinrulesrc")
}

/// 启用/停用“跳过任务栏 + 跳过 Alt-Tab”规则（幂等）。
pub fn set_skip_taskbar(enabled: bool) {
    if !is_kde() {
        if enabled {
            log::info!("非 KDE 会话，跳过任务栏规则不适用（Windows/X11 可直接隐藏窗口）");
        }
        return;
    }
    let path = rules_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let had_rule = text.contains(MARKER);
    if enabled == had_rule {
        return; // 已是目标状态
    }
    let new_text = if enabled { add_rule(&text) } else { remove_rule(&text) };
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
        Ok(_) => log::info!("已{} KWin「跳过任务栏」规则", if enabled { "启用" } else { "移除" }),
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

/// 在 INI 文本中追加本程序规则（若不存在）
fn add_rule(text: &str) -> String {
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
    for (name, body) in &groups {
        out.push_str(&format!("\n[{name}]\n{body}"));
    }
    out.push_str(&format!(
        "\n[{id}]\nDescription={MARKER}\nskiptaskbar=true\nskiptaskbarrule=2\nskipswitcher=true\nskipswitcherrule=2\nwmclass={WM_CLASS}\nwmclassmatch=1\nwmclasscomplete=false\ntypes=1\n"
    ));
    out
}

/// 从 INI 文本中删除本程序规则（若存在），保留其他规则
fn remove_rule(text: &str) -> String {
    let (mut general, groups) = split_ini(text);
    let kept: Vec<(String, String)> = groups.iter().filter(|(_, body)| !body.contains(MARKER)).cloned().collect();
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


/// 显示本程序窗口：取消最小化并激活（KDE/Wayland 下 winit 无法做到，交给 KWin 脚本）。
///
/// winit 在 Wayland 下无法程序化恢复最小化的窗口（focus_window / set_minimized(false) 均为空实现），
/// 而 KWin 脚本可以做到（同一个脚本里先取消最小化再赋值 activeWindow 即可，实测可靠）。
/// 非 KDE 会话返回 false。
pub fn unminimize_activate(keep_on_top: bool) -> bool {
    if !is_kde() {
        return false;
    }
    // 取消最小化 + 激活；keep_on_top 时额外置顶（激活是否被接受由 KWin 决定，不稳定，置顶可靠）
    let show = format!(
        r#"
function run() {{
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {{
        var w = wins[i];
        if (String(w.resourceClass).indexOf("kmcounter") >= 0) {{
            w.minimized = false;
            w.keepAbove = {keep};
            workspace.activeWindow = w;
        }}
    }}
}}
run();
"#,
        keep = if keep_on_top { "true" } else { "false" }
    );
    // KWin 是否接受脚本激活并不稳定（实测同一脚本时好时坏），多试几次提高成功率
    let act = r#"
function run() {
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {
        var w = wins[i];
        if (String(w.resourceClass).indexOf("kmcounter") >= 0) { workspace.activeWindow = w; }
    }
}
run();
"#;
    let ok = run_script("kmc-show", &show);
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        run_script("kmc-act", act);
    }
    if ok {
        log::info!("已通过 KWin 脚本显示窗口（取消最小化 + 激活{}）", if keep_on_top { " + 置顶" } else { "" });
    }
    ok
}

/// 收起窗口时清除置顶（避免窗口一直压在其他窗口上）
pub fn clear_keep_above() {
    if !is_kde() {
        return;
    }
    let script = r#"
function run() {
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {
        var w = wins[i];
        if (String(w.resourceClass).indexOf("kmcounter") >= 0) { w.keepAbove = false; }
    }
}
run();
"#;
    run_script("kmc-unkeep", script);
}

/// 把一段 KWin 脚本写入临时文件并让 KWin 加载执行。
///
/// 两个 KWin 脚本接口的坑（都踩过并已修复）：
/// 1. **脚本名必须每次唯一**：Scripting.start() 只执行「尚未启动过」的脚本，同名第二次调用会静默跳过；
/// 2. **不要卸载刚加载的脚本**：KWin 的执行是异步的，立即卸载有竞态（会把还没执行的脚本取消掉），
///    因此这里只卸载「上一次」的脚本（此时它必然已执行完），避免脚本对象累积。
fn run_script(tag: &str, body: &str) -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = format!("{tag}-{}-{}", std::process::id(), seq);
    let path = std::env::temp_dir().join(format!("{name}.js"));
    if let Err(e) = std::fs::write(&path, body) {
        log::warn!("写入 KWin 脚本失败: {e}");
        return false;
    }
    let res = (|| -> Result<(), String> {
        let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
        let p = path.to_string_lossy().to_string();
        conn.call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "loadScript",
            &(p.as_str(), name.as_str()),
        )
        .map_err(|e| e.to_string())?;
        conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), "start", &())
            .map_err(|e| e.to_string())?;
        static PREV: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
        if let Some(prev) = PREV.lock().unwrap().replace(name.clone()) {
            let _ = conn.call_method(
                Some("org.kde.KWin"),
                "/Scripting",
                Some("org.kde.kwin.Scripting"),
                "unloadScript",
                &(prev.as_str(),),
            );
        }
        Ok(())
    })();
    match res {
        Ok(_) => true,
        Err(e) => {
            log::warn!("KWin 脚本({tag})执行失败: {e}");
            false
        }
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
}
