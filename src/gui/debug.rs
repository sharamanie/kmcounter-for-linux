//! 软渲染（softshot）出图与测试用的 getter / setter —— 从 gui.rs 拆出（第一步纯搬运）。
//!
//! 每个方法都带 softshot feature 条件编译属性，不参与正式构建。

use super::*;

impl App {
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_set_layout(&mut self, id: &str) {
        self.cfg.layout.keyboard_layout = id.to_string();
        self.edit.layout.keyboard_layout = id.to_string();
        self.cfg_shared.lock().layout.keyboard_layout = id.to_string();
    }

    /// 布局锚点（软渲染出图时用来确定画布高度）
    #[cfg(feature = "softshot")]
    pub(crate) fn debug_layout(&self) -> (f32, f32, f32, f32) {
        (self.base_h, self.panel_extra_h(), self.content_bottom, self.bottom_region_top)
    }

    /// 测试用 setter：不通过环境变量（环境变量是进程级的，并行测试会互相串）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_hour(&mut self, hour: Option<u8>) {
        self.hour_sel = hour;
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_open_panel(&mut self) {
        self.more_open = true;
    }

    /// 指定「当前显示数据」的视图（0 = 总计，1 = 今日，2 = 昨天…），截图用
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_view_idx(&mut self, idx: usize) {
        self.view_idx = idx;
        self.rebuild_view_days();
        self.title_dirty = true;
    }

    /// 直接设置已选按键（截图用：高亮几个真实高频键）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_select(&mut self, keys: &[usize]) {
        self.selected = keys.iter().copied().collect();
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_trend_page(&mut self) {
        self.page = Page::Trends;
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_nav_title_rect(&self) -> Rect {
        self.nav_title_rect
    }

    /// 当前语言的文案表（测试里按文案定位控件）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_strings(&self) -> &'static crate::lang::Strings {
        self.s()
    }

    /// 系统里有没有中文字体（测试据此决定是否检查中文界面）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_has_cjk(&self) -> bool {
        self.has_cjk
    }

    /// 指定屏幕物理尺寸（测试里让「屏幕尺寸」有一个确定的数值）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_screen_mm(&self, w: u32, h: u32) {
        self.shared.screen_w_mm.store(w, Ordering::Relaxed);
        self.shared.screen_h_mm.store(h, Ordering::Relaxed);
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_open_calendar(&mut self) {
        self.cal_open = true;
        let d = chrono::Local::now().date_naive();
        self.cal_month = (d.year(), d.month());
    }

    /// 视图标签 / 跳转某天 / 托盘标志（离屏测试用）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_view_label(&self) -> String {
        self.view_label()
    }

    /// 供测试直接走「日历点某天」的同一条路径
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_goto_day(&mut self, date: &str) -> bool {
        self.goto_day(date)
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_calendar_open(&self) -> bool {
        self.cal_open
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_flags(&self) -> Arc<crate::controller::UiFlags> {
        self.flags.clone()
    }

    /// 分布条矩形 / 当前选中小时 / 已选按键（离屏交互测试用）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_strip_rect(&self) -> Rect {
        self.strip_rect
    }

    /// 排行页：切页与筛选（测试用，不走环境变量）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_rank(&mut self, range: RankRange, scopes: &[RankScope], metric: RankMetric, desc: bool) {
        self.rank_range = range;
        self.rank_scopes = scopes.iter().copied().collect();
        self.rank_metric = metric;
        self.rank_desc = desc;
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_open_rank_page(&mut self) {
        self.page = Page::Ranking;
    }

    /// 调试：关闭右上角 "?" 的说明页（测试用）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_close_help(&mut self) {
        self.help_open = false;
    }

    /// 调试：等同于点一下右上角 "?"（走 `toggle_help`，与真实点击同一份逻辑）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_toggle_help(&mut self) {
        self.toggle_help();
    }

    /// 调试：底部「展开更多」是否展开
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_more_open(&self) -> bool {
        self.more_open
    }

    /// 调试：是否处于帮助页
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_help_open(&self) -> bool {
        self.help_open
    }

    /// 调试：展开 HUD 设置窗（离屏核对设置窗排版用；`which` 含 "apm"/"kps"）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_open_hud_settings(&mut self, which: &str) {
        if which.contains("apm") {
            self.apm_cfg_open = true;
        }
        if which.contains("kps") {
            self.kps_cfg_open = true;
        }
    }

    /// 展开右上角 "?" 的悬浮窗设置说明（截图 / 离屏核对文字用）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_open_help(&mut self) {
        self.help_open = true;
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_page_is_rank(&self) -> bool {
        self.page == Page::Ranking
    }

    /// 解停放相关状态（测试用）：是否被判定为用户手动缩放、当前待下发的目标高度
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_user_sized(&self) -> bool {
        self.user_sized
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_resize_target(&self) -> Option<f32> {
        self.resize_target
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_resize_target_w(&self) -> Option<f32> {
        self.resize_target_w
    }

    /// 模拟收起到托盘 / 从托盘呼出（跨线程状态，测试里直接设）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_ui_hidden(&self, hidden: bool) {
        self.shared.ui_hidden.store(hidden, Ordering::Relaxed);
    }

    /// 导航行日期标题的固定宽度（测试核对区间文字放得下）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_nav_title_w(&self) -> f32 {
        NAV_TITLE_W
    }

    /// 导航行现在显示的文案（区间 / 单日 / 总计）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_nav_label(&self) -> String {
        self.nav_display_label()
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_hour_controls_enabled(&self) -> bool {
        self.hour_controls_enabled()
    }

    /// 排行页当前算出来的统计区间
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_rank_window(&self) -> (NaiveDate, NaiveDate) {
        self.rank_window()
    }

    /// 排行页右上角分时折线图的矩形
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_rank_chart_rect(&self) -> Rect {
        self.rank_chart_rect
    }

    /// 排行列表的可见矩形（测试按行高算某一行位置）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_rank_rect(&self) -> Rect {
        self.rank_rect
    }

    /// 当前筛选下算出来的排行（测试直接核对数值与顺序）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_rank_rows(&self) -> Vec<(String, u64, Option<usize>, bool)> {
        let (from, to) = self.rank_window();
        let only = self.rank_only_selected.then_some(&self.selected);
        let mut rows = rank_rows(&self.snap, from, to, self.hour_sel, &self.rank_scopes, only, self.s());
        let m = self.rank_metric;
        rows.sort_by(|a, b| {
            let (va, vb) = (rank_value(a, m), rank_value(b, m));
            let ord = vb.partial_cmp(&va).unwrap_or(std::cmp::Ordering::Equal);
            let ord = if self.rank_desc { ord } else { ord.reverse() };
            ord.then_with(|| a.label.cmp(&b.label))
        });
        rows.into_iter().map(|r| (r.label, r.count, r.key_idx, r.mouse)).collect()
    }

    /// 键盘区域矩形与布局步长（测试定位鼠标五键）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_kbd_rect(&self) -> Rect {
        self.kbd_rect
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_metrics(&self) -> (f32, f32, f32) {
        let (u, pitch, gap, _) = self.current_metrics();
        (u, pitch, gap)
    }

    /// 趋势图矩形（测试按桶中心点击）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_chart_rect(&self) -> Rect {
        self.chart_rect
    }

    /// 切换趋势粒度（测试里不通过环境变量）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_granularity(&mut self, g: Granularity) {
        self.granularity = g;
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_hour_sel(&self) -> Option<u8> {
        self.hour_sel
    }

    /// 走“保存”同一条路径切换语言，返回生效后的语言代码（离屏测试用）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_language(&mut self, code: &str) -> &'static str {
        self.edit.language = code.to_string();
        self.apply_language();
        match self.lang {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_selected(&self) -> Vec<usize> {
        self.selected.iter().copied().collect()
    }
}
