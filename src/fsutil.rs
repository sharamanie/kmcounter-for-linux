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
}
