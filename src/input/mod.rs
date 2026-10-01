//! 输入捕获层。生产者（平台钩子/evdev）把原始事件发给 applier 线程，
//! applier 统一写入共享统计（加锁、跨夜滚动）。

#[cfg(windows)]
pub mod windows;
#[cfg(target_os = "linux")]
pub mod linux;

use crate::stats::{MouseEv, Shared};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

#[derive(Debug)]
pub enum InEvent {
    /// 布局内按键（索引）
    Key(usize),
    /// 布局外按键
    KeyOther(String),
    Mouse(MouseEv),
}

/// 启动 applier 线程，返回生产者用的发送端
pub fn spawn_applier(shared: Arc<Shared>) -> Sender<InEvent> {
    let (tx, rx): (Sender<InEvent>, Receiver<InEvent>) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("kmcounter-applier".into())
        .spawn(move || {
            while let Ok(ev) = rx.recv() {
                shared.rollover_if_needed();
                let date = shared.today.lock().clone();
                let hour = shared.current_hour();
                let mut store = shared.store.lock();
                match ev {
                    InEvent::Key(idx) => store.bump_key(&date, hour, idx),
                    InEvent::KeyOther(id) => store.bump_other_key(&date, hour, &id),
                    InEvent::Mouse(m) => store.bump_mouse(&date, hour, &m),
                }
                drop(store);
                shared.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        })
        .expect("无法启动 applier 线程");
    tx
}

/// 启动平台输入捕获
pub fn start(shared: Arc<Shared>) {
    #[cfg(windows)]
    windows::start(shared);
    #[cfg(target_os = "linux")]
    linux::start(shared);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        shared.set_input_status(false, "此平台暂不支持输入捕获".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Store;
    use std::time::Duration;

    /// 端到端：事件经 applier 落库时同时写进“该小时”桶与总计分时桶
    #[test]
    fn applier_writes_hour_buckets() {
        let shared = Shared::new(Store::default(), 0, 0);
        let tx = spawn_applier(shared.clone());
        tx.send(InEvent::Key(5)).unwrap();
        tx.send(InEvent::Mouse(MouseEv::MovePx(10.0))).unwrap();
        drop(tx); // 关闭通道，applier 处理完剩余事件后退出

        for _ in 0..100 {
            if shared.store.lock().total.mouse.move_px >= 10.0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let store = shared.store.lock();
        let today = shared.today.lock().clone();
        let hour = shared.current_hour();
        assert_eq!(store.day(&today).expect("应有今日数据").key_count(5), 1);
        assert_eq!(store.hour(&today, hour).expect("应有该小时数据").key_count(5), 1);
        assert_eq!(store.hour(&today, hour).unwrap().mouse.move_px, 10.0);
        assert_eq!(store.total.hour(hour).expect("总计也应有分时").keystrokes, 1);
        // 只有一个小时桶有数据，其它小时不应凭空建桶
        assert_eq!(store.day(&today).unwrap().hours.len(), 1);
    }
}
