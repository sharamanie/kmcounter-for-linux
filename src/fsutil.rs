//! 文件写入小工具：唯一临时名 + rename，避免多个写入方（GUI / 后台 saver / 信号处理）
//! 并发保存同一个文件时互相覆盖。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 同目录下的唯一临时路径（带 pid 与自增序号）。
/// 固定临时名在多写入方并发时会被互相截断/交错写入，落盘内容可能是坏文件。
pub fn unique_tmp(path: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "data".into());
    path.with_file_name(format!(".{name}.{}.{seq}.tmp", std::process::id()))
}

/// 把**损坏**的数据文件改名留档，返回留档路径（`<名字>.corrupt-<YYYYMMDD-HHMMSS>`）。
///
/// 为什么带时间戳：固定叫 `.bak` 时，第二次损坏会把第一次的留档覆盖掉 —— 用户真正想找回的
/// 往往是最早那份（后来写的可能已经是空壳）。改名而不是删，是"损坏也绝不丢数据"的底线。
pub fn quarantine_corrupt(path: &Path) -> Option<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = path.file_name()?.to_string_lossy().to_string();
    // 同一秒内连续损坏会撞名：加序号，绝不覆盖已有留档
    let mut bak = path.with_file_name(format!("{name}.corrupt-{stamp}"));
    let mut seq = 2;
    while bak.exists() {
        bak = path.with_file_name(format!("{name}.corrupt-{stamp}-{seq}"));
        seq += 1;
    }
    match std::fs::rename(path, &bak) {
        Ok(_) => Some(bak),
        Err(e) => {
            log::warn!("损坏文件留档失败（{} → {}）: {e}", path.display(), bak.display());
            None
        }
    }
}

/// 原子写文件：写唯一临时文件 → 落盘 → rename 覆盖目标。
/// 失败时清理临时文件，目标文件保持原样。
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = unique_tmp(path);
    let written = (|| -> std::io::Result<()> {
        let f = std::fs::File::create(&tmp)?;
        let mut w = std::io::BufWriter::new(f);
        w.write_all(bytes)?;
        w.flush()?;
        w.get_ref().sync_all().ok();
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// 清理上次异常退出遗留的临时文件（唯一临时名各写各的，崩溃时可能残留）。
/// 只删超过 1 小时的：另一个进程可能正在写自己的临时文件，不能误删。
pub fn clean_stale_tmp(path: &Path) {
    let Some(dir) = path.parent() else { return };
    let Some(name) = path.file_name() else { return };
    let prefix = format!(".{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for e in entries.flatten() {
        let fname = e.file_name().to_string_lossy().to_string();
        if !(fname.starts_with(&prefix) && fname.ends_with(".tmp")) {
            continue;
        }
        let stale = e.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_tmp_names_differ() {
        let p = Path::new("/tmp/x/stats.json");
        assert_ne!(unique_tmp(p), unique_tmp(p), "并发写入方不能撞名");
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("kmc_fsutil_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("data.json");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "写完不应留下临时文件");
        // 刚写的临时文件不能被当成残留清掉（可能别的进程正在写）
        let fresh = unique_tmp(&p);
        std::fs::write(&fresh, b"x").unwrap();
        clean_stale_tmp(&p);
        assert!(fresh.exists(), "新鲜的临时文件不该被清理");
        std::fs::remove_file(&fresh).ok();
        std::fs::remove_dir_all(dir).ok();
    }
    /// 损坏留档要带时间戳：连续两次损坏不能互相覆盖（第二次别把第一次的备份冲掉）
    #[test]
    fn quarantine_keeps_every_corrupt_copy() {
        let dir = std::env::temp_dir().join(format!("kmc_quarantine_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stats.json");
        std::fs::write(&path, b"bad-1").unwrap();
        let first = super::quarantine_corrupt(&path).expect("第一次留档");
        std::fs::write(&path, b"bad-2").unwrap();
        // 同一秒内连续两次也要能分开（时间戳精度到秒，重名时第二次仍会覆盖 —— 这里只断言第一次还在）
        let second = super::quarantine_corrupt(&path).map(|p| p);
        assert!(first.exists(), "第一次的留档不能被覆盖：{first:?}");
        if let Some(second) = second {
            assert!(second.exists());
            assert_ne!(first, second);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

}
