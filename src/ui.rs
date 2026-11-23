//! 界面：浮动红绿灯、漫画视图、底部液态玻璃缩略图条。

use crate::book::Book;
use crate::decode::{Decoder, Kind};
use egui::{
    Align2, Color32, CornerRadius, FontId, Mesh, Pos2, Rect, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// 底部那条白色拉手：高度、收起时离窗口底边的距离
const HANDLE_H: f32 = 6.0;
const HANDLE_REST: f32 = 26.0;
/// 抽屉拉到这个高度以下就当作没收起（松手直接弹回）
const DRAWER_MIN: f32 = 92.0;
const THUMB_GAP: f32 = 10.0;
/// 左侧"最近打开"保留几条
const RECENT_SLOTS: usize = 6;
/// 「最近打开」一条一行的高度：滚动按整行对齐，顶上半截的黑杠就不会出现
const RECENT_ROW_H: f32 = 88.0;

#[derive(Default)]
pub struct Actions {
    pub close: bool,
    pub minimize: bool,
    pub maximize: bool,
    pub drag: bool,
    pub open_dialog: bool,
    pub goto: Option<usize>,
    pub open_path: Option<PathBuf>,
    pub toggle_theme: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// 左侧近期记录：图片或压缩包，封面是各自第一页
struct RecentItem {
    path: PathBuf,
    title: String,
    kind: &'static str,
    cover: Option<TextureHandle>,
}

struct CoverArt {
    path: PathBuf,
    size: [u32; 2],
    rgba: Vec<u8>,
}

pub struct Viewer {
    pub book: Option<Arc<Book>>,
    book_id: u64,
    pub current: usize,
    decoder: Decoder,

    full: HashMap<usize, TextureHandle>,
    thumbs: HashMap<usize, TextureHandle>,
    thumb_index: usize,
    thumb_budget: usize,

    strip_scroll: f32,
    strip_velocity: f32,
    /// 底部抽屉：白条底边到窗口底边的距离，0 就是收起
    drawer: f32,
    drawer_target: f32,
    drawer_drag: bool,
    drawer_grab: f32,
    /// 本帧缩略图高度，跟着抽屉高度走
    thumb_h: f32,
    /// 点抽屉两侧小箭头时，缩略图条平滑滚向的目标位置
    strip_target: Option<f32>,
    /// 刚打开一本书时，把当前页的缩略图滚到中间一次（之后点缩略图不再自动跳）
    strip_center_once: bool,
    status: String,

    /// 左侧"最近打开"
    recent: Vec<RecentItem>,
    recent_open: bool,
    recent_scroll: f32,
    /// 滚动目标（未吸附），实际位置缓动追它，停下来时按整行对齐
    recent_target: f32,
    /// 按住列表往下拖：记下按下时的指针 y 和当时的滚动量
    recent_drag: Option<(f32, f32)>,
    /// 这一拖已经超过阈值，抬手时就不算点条目了
    recent_dragged: bool,
    /// 点了条目之后先留着的透明度，1 = 不透明
    recent_fade: f32,
    /// 渐隐前的等待（秒）：等右侧新书出现后再开始倒数
    recent_hold: f32,
    /// 点了条目但新书还没出来，等它出来再开始倒数
    recent_wait: Option<PathBuf>,
    /// 正在渐隐
    recent_closing: bool,
    /// 刚点的那一条，留着高亮给点反馈
    recent_active: Option<PathBuf>,
    /// 面板开着时点过的那条，等面板收起再提到最前
    recent_pending: Option<PathBuf>,
    /// `--demo-recent`：自动开面板并点第二条，给延时渐隐做验收用
    demo: bool,
    demo_start: std::time::Instant,
    demo_fired: bool,
    cover_tx: crossbeam_channel::Sender<CoverArt>,
    cover_rx: crossbeam_channel::Receiver<CoverArt>,

    theme: Theme,
    /// 当前整页图占的矩形，用来判断右键是否落在空白处
    page_rect: Option<Rect>,
    /// 双页模式下右边那张
    page_rect2: Option<Rect>,
    /// 单页 / 双页
    spread: bool,
    /// 双页里左右对调的进度：0 = 正常，1 = 对调
    spread_t: f32,
    /// 对调动画要去的地方
    spread_t_to: f32,
    /// 这次对调里滑过去的是哪一张（画在最上面）
    spread_mover: Option<usize>,
    /// 双页拖拽：拖的是哪张、起手 x、相对位移（按两张槽位间距算的比例）
    spread_drag: Option<SpreadDrag>,
    /// 直接从 winit 记下来的指针：合成输入下 egui 的 pointer 不更新，拖拽不能指望它
    raw_pos: Option<Pos2>,
    raw_press: bool,
    raw_release: bool,
    /// `--demo-spread`：自动切双页并对调一次，给验收用
    demo_spread: bool,
    demo_spread_hold: bool,
    demo_spread_fired: bool,
    demo_spread_fired2: bool,
    /// 第一帧的时间，演示计时从这里算，别把启动耗时也算进去
    demo_spread_at: Option<std::time::Instant>,
    /// `--demo-drag`：用和真鼠标同样的通道，连做两轮双页拖拽
    demo_drag: bool,
    demo_drag_at: Option<std::time::Instant>,
    demo_drag_step: u8,
    demo_drag_anchor: Pos2,
    demo_drag_span: f32,
    /// 按帧计数的进度：窗口被遮住时帧会很稀，用时间轴跑不可靠
    demo_drag_frames: u32,
}

impl Default for Viewer {
    fn default() -> Self {
        Self::new()
    }
}

/// 双页里正在被按住拖的那张：页码、起手 x、起手时的左右朝向、当前位移比例
struct SpreadDrag {
    idx: usize,
    start_x: f32,
    /// 起手时这张在左边吗
    from_left: bool,
    /// 起手时的对调进度（0 或 1）
    base: f32,
    /// 当前位移，按「两个槽位中心之间的距离」归一
    f: f32,
}

/// 松手时的换算：返回（这一帧的进度，动画要去的目标）。
/// `base` 是起手时的进度，`idx` 是抓的那张的页码，`current` 是当前页。
/// 抓的这张往对面拖够 14%，就对调；否则弹回起手位置。
/// 抽成纯函数是为了能按真实手势序列跑单测（见 tests::spread_swap_repeats）。
fn spread_release(base: f32, idx: usize, current: usize, from_left: bool, f: f32) -> (f32, f32) {
    // a（当前页）往右走 t 增，b 往右走 t 减
    let dir = if idx == current { 1.0 } else { -1.0 };
    let p = (base + dir * f).clamp(0.0, 1.0);
    let toward = if from_left { f > 0.0 } else { f < 0.0 };
    let past = f.abs() >= 0.14 && toward;
    // 目标永远落在两端的 0 或 1 上：动画中途按下去，base 可能是 0.8 这种小数
    (p, if past { if base > 0.5 { 0.0 } else { 1.0 } } else { base })
}

impl Viewer {
    pub fn new() -> Self {
        let (cover_tx, cover_rx) = crossbeam_channel::unbounded();
        // 上次留下的最近记录（只认还在磁盘上的路径）
        let mut recent = Vec::new();
        if let Some(f) = recent_store_path() {
            if let Ok(s) = std::fs::read_to_string(&f) {
                for line in s.lines().filter(|l| !l.trim().is_empty()).take(RECENT_SLOTS) {
                    let raw = PathBuf::from(line);
                    if !raw.exists() {
                        continue;
                    }
                    let path = std::fs::canonicalize(&raw).map(norm_path).unwrap_or(raw);
                    let title = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("untitled")
                        .to_string();
                    let kind = if path.is_dir() {
                        "文件夹"
                    } else if is_archive(&path) {
                        "压缩包"
                    } else {
                        "图片"
                    };
                    if recent.iter().any(|r: &RecentItem| r.path == path) {
                        continue;
                    }
                    recent.push(RecentItem {
                        path,
                        title,
                        kind,
                        cover: None,
                    });
                }
            }
        }
        let v = Self {
            book: None,
            book_id: 0,
            current: 0,
            decoder: Decoder::new(),
            full: HashMap::new(),
            thumbs: HashMap::new(),
            thumb_index: 0,
            thumb_budget: 900,
            strip_scroll: 0.0,
            strip_velocity: 0.0,
            drawer: 0.0,
            drawer_target: 0.0,
            drawer_drag: false,
            drawer_grab: 0.0,
            thumb_h: 58.0,
            strip_target: None,
            strip_center_once: false,
            status: String::new(),
            recent,
            recent_open: false,
            recent_scroll: 0.0,
            recent_target: 0.0,
            recent_drag: None,
            recent_dragged: false,
            recent_fade: 1.0,
            recent_hold: 0.0,
            recent_wait: None,
            recent_closing: false,
            recent_active: None,
            recent_pending: None,
            demo: std::env::args().any(|a| a == "--demo-recent"),
            demo_start: std::time::Instant::now(),
            demo_fired: false,
            cover_tx,
            cover_rx,
            theme: Theme::Light,
            page_rect: None,
            page_rect2: None,
            spread: std::env::args().any(|a| {
                a == "--spread" || a == "--demo-spread" || a == "--demo-spread-hold"
            }),
            spread_t: 0.0,
            spread_t_to: 0.0,
            spread_mover: None,
            spread_drag: None,
            raw_pos: None,
            raw_press: false,
            raw_release: false,
            demo_spread: std::env::args().any(|a| a == "--demo-spread" || a == "--demo-spread-hold"),
            demo_spread_hold: std::env::args().any(|a| a == "--demo-spread-hold"),
            demo_spread_fired: false,
            demo_spread_fired2: false,
            demo_spread_at: None,
            demo_drag: std::env::args().any(|a| a == "--demo-drag"),
            demo_drag_at: None,
            demo_drag_step: 0,
            demo_drag_anchor: Pos2::ZERO,
            demo_drag_span: 400.0,
            demo_drag_frames: 0,
        };
        let seeds: Vec<PathBuf> = v.recent.iter().map(|r| r.path.clone()).collect();
        for p in seeds {
            spawn_cover(v.cover_tx.clone(), p);
        }
        v
    }

    pub fn open(&mut self, path: &std::path::Path) {
        // 统一成绝对路径：最近记录、封面缓存的 key 才不会因为相对/绝对写法不同而各存一份
        let path = std::fs::canonicalize(path)
            .map(norm_path)
            .unwrap_or_else(|_| path.to_path_buf());
        match Book::open(&path) {
            Ok(b) => {
                let title = b.title.clone();
                let kind = if path.is_dir() {
                    "文件夹"
                } else if is_archive(&path) {
                    "压缩包"
                } else {
                    "图片"
                };
                let book = Arc::new(b);
                self.book_id = self.decoder.new_book();
                self.full.clear();
                self.thumbs.clear();
                // 单图打开时 pages 是整目录的图，当前页要定在用户点的那张上
                let key = path.to_string_lossy().to_string();
                let start = book.pages.iter().position(|p| p == &key).unwrap_or(0);
                self.thumb_index = start;
                self.current = start;
                self.strip_scroll = 0.0;
                self.strip_target = None;
                self.strip_center_once = start > 0;
                self.status.clear();
                // 换书不动左右阅读朝向：漫画的「右开」是要一直用的设置
                self.spread_drag = None;
                self.book = Some(book);
                self.note_recent(path.clone(), title, kind);
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// 记一条最近打开，最多留 RECENT_SLOTS 条，封面在后台解第一页
    fn note_recent(&mut self, path: PathBuf, title: String, kind: &'static str) {
        // 面板开着的时候顺序不许动（用户刚点的那条会跳上去，看着像列表在抽风），
        // 先只更新信息、记下待提升，等面板收起再把它提到最前。
        let open = self.recent_open;
        if let Some(i) = self.recent.iter().position(|r| r.path == path) {
            let need_cover = {
                let item = &mut self.recent[i];
                item.title = title;
                item.kind = kind;
                item.cover.is_none()
            };
            if need_cover {
                spawn_cover(self.cover_tx.clone(), path.clone());
            }
            if open {
                self.recent_pending = Some(path);
            } else if i > 0 {
                let item = self.recent.remove(i);
                self.recent.insert(0, item);
                self.recent.truncate(RECENT_SLOTS);
            }
        } else {
            self.recent.insert(
                0,
                RecentItem {
                    path: path.clone(),
                    title,
                    kind,
                    cover: None,
                },
            );
            if open {
                // 面板开着先别裁，等收起时 promote 一并处理
                self.recent_pending = Some(path.clone());
            } else {
                self.recent.truncate(RECENT_SLOTS);
            }
            spawn_cover(self.cover_tx.clone(), path);
        }
        self.store_recent();
    }

    /// 把「面板开着时点过的那条」提到最前，并裁到 RECENT_SLOTS 条
    fn promote_pending(&mut self) {
        let Some(p) = self.recent_pending.take() else { return };
        if let Some(i) = self.recent.iter().position(|r| r.path == p) {
            if i > 0 {
                let item = self.recent.remove(i);
                self.recent.insert(0, item);
            }
        }
        self.recent.truncate(RECENT_SLOTS);
        self.store_recent();
    }

    fn store_recent(&self) {
        let Some(f) = recent_store_path() else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let body = self
            .recent
            .iter()
            .map(|r| r.path.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(f, body);
    }

    fn pump_covers(&mut self, ctx: &egui::Context) {
        while let Ok(c) = self.cover_rx.try_recv() {
            let Some(item) = self.recent.iter_mut().find(|r| r.path == c.path) else {
                continue;
            };
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [c.size[0] as usize, c.size[1] as usize],
                &c.rgba,
            );
            item.cover = Some(ctx.load_texture(
                format!("cover-{}", item.path.display()),
                img,
                TextureOptions::LINEAR,
            ));
        }
        // 封面是后台线程解的，没到之前每 250ms 主动刷一帧，别让面板停在占位
        if self.recent.iter().any(|r| r.cover.is_none()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn total(&self) -> usize {
        self.book.as_ref().map(|b| b.len()).unwrap_or(0)
    }

    pub fn goto_public(&mut self, idx: usize) {
        self.goto(idx);
    }

    pub fn next_public(&mut self) {
        self.next();
    }

    pub fn total_public(&self) -> usize {
        self.total()
    }

    pub fn theme_is_light(&self) -> bool {
        self.theme == Theme::Light
    }

    pub fn toggle_theme(&mut self) {
        self.theme = if self.theme == Theme::Light {
            Theme::Dark
        } else {
            Theme::Light
        };
    }

    fn is_light(&self) -> bool {
        self.theme == Theme::Light
    }

    /// 文字/线条：浅色界面用黑，深色界面用白
    fn ink(&self, a: u8) -> Color32 {
        if self.is_light() {
            Color32::from_black_alpha(a)
        } else {
            Color32::from_white_alpha(a)
        }
    }

    /// 淡淡的填充（hover 底、轨道）
    fn veil(&self, a: u8) -> Color32 {
        if self.is_light() {
            Color32::from_black_alpha((a as f32 * 0.70) as u8)
        } else {
            Color32::from_white_alpha(a)
        }
    }

    /// 文字后面的实底，保证在图片上也读得清
    fn plate(&self, a: u8) -> Color32 {
        if self.is_light() {
            Color32::from_rgba_unmultiplied(250, 250, 252, a)
        } else {
            Color32::from_rgba_unmultiplied(8, 8, 11, a)
        }
    }

    fn goto(&mut self, idx: usize) {
        let total = self.total();
        if total == 0 {
            return;
        }
        let idx = idx.min(total - 1);
        if idx != self.current {
            self.current = idx;
            self.decoder.bump();
            self.strip_velocity = 0.0;
        }
    }

    fn next(&mut self) {
        let step = if self.spread && self.total() > 1 { 2 } else { 1 };
        if self.current + 1 < self.total() {
            self.goto((self.current + step).min(self.total() - 1));
        }
    }

    fn prev(&mut self) {
        let step = if self.spread && self.total() > 1 { 2 } else { 1 };
        self.goto(self.current.saturating_sub(step));
    }

    fn pump(&mut self, ctx: &egui::Context) {
        self.pump_covers(ctx);
        for d in self.decoder.drain() {
            if d.book_id != self.book_id {
                continue;
            }
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied([d.size[0] as usize, d.size[1] as usize], &d.rgba);
            match d.kind {
                Kind::Full => {
                    let tex = ctx.load_texture(
                        format!("full-{}-{}", self.book_id, d.idx),
                        color_image,
                        TextureOptions::LINEAR,
                    );
                    self.full.insert(d.idx, tex);
                    // 只留当前页附近，避免上千页纹理把显存吃光
                    let lo = self.current.saturating_sub(9);
                    let hi = self.current + 9;
                    self.full.retain(|k, _| *k >= lo && *k <= hi);
                }
                Kind::Thumb => {
                    let tex = ctx.load_texture(
                        format!("thumb-{}-{}", self.book_id, d.idx),
                        color_image,
                        TextureOptions::LINEAR,
                    );
                    self.thumbs.insert(d.idx, tex);
                }
            }
        }

        // 预取：当前页优先，前后各若干页；缩略图从前往后填
        if let Some(book) = self.book.clone() {
            let total = book.len();
            let cur = self.current;
            if !self.full.contains_key(&cur) {
                self.decoder.request_full(&book, cur, self.book_id);
            }
            for k in 1..=4usize {
                if cur + k < total && !self.full.contains_key(&(cur + k)) {
                    self.decoder.request_full(&book, cur + k, self.book_id);
                }
                if cur >= k && !self.full.contains_key(&(cur - k)) {
                    self.decoder.request_full(&book, cur - k, self.book_id);
                }
            }

            // 缩略图：优先当前页附近，然后按顺序补齐，限额防止一次塞满队列
            let mut budget = 12;
            for k in 0..40usize {
                for idx in [cur.wrapping_sub(k), cur + k] {
                    if idx < total && !self.thumbs.contains_key(&idx) && budget > 0 {
                        self.decoder.request_thumb(&book, idx, self.book_id);
                        budget -= 1;
                    }
                }
                if budget == 0 {
                    break;
                }
            }
            let mut placed = 0;
            while self.thumb_index < total && placed < 8 {
                if !self.thumbs.contains_key(&self.thumb_index) {
                    self.decoder.request_thumb(&book, self.thumb_index, self.book_id);
                    placed += 1;
                }
                self.thumb_index += 1;
            }
            if self.thumb_index >= total {
                self.thumb_index = 0;
            }
        }
    }

    pub fn ui(&mut self, root: &mut egui::Ui) -> Actions {
        let ctx = root.ctx().clone();
        let mut actions = Actions::default();
        self.pump(&ctx);
        // `--demo-spread`：1.4s 和 3.2s 各替用户做一次对调（连做两次，验能不能反复换），
        // `--demo-spread-hold` 则停在动画正中间，方便截图核对
        if self.demo_spread && !self.demo_spread_fired {
            let at = *self.demo_spread_at.get_or_insert_with(std::time::Instant::now);
            if at.elapsed() > std::time::Duration::from_millis(1400) {
                if self.demo_spread_hold {
                    // 卡在动画正中间，专门给截图看「一张在上面走、一张缩着让路」
                    self.spread_t = 0.5;
                    self.spread_t_to = 0.5;
                    self.spread_mover = Some(self.current);
                } else {
                    self.start_spread_swap(self.current);
                }
                self.demo_spread_fired = true;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        if self.demo_spread && self.demo_spread_fired && !self.demo_spread_fired2 {
            let at = *self.demo_spread_at.get_or_insert_with(std::time::Instant::now);
            if at.elapsed() > std::time::Duration::from_millis(3200) {
                self.start_spread_swap(self.current + 1);
                self.demo_spread_fired2 = true;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        self.tick_spread_anim(&ctx);
        if self.demo_drag {
            self.demo_drag_tick(&ctx);
        }
        self.handle_input(&ctx, &mut actions);
        let screen = root.max_rect();
        let painter = root.painter().clone();
        self.page_rect = None;
        if self.is_light() {
            painter.rect_filled(screen, 0.0, Color32::from_rgb(241, 241, 245));
            vertical_gradient(
                &painter,
                screen,
                Color32::from_rgb(248, 248, 251),
                Color32::from_rgb(232, 232, 238),
            );
        } else {
            // 近乎纯黑的底色，只在中间留一点点提亮，像暗房里的一束光
            painter.rect_filled(screen, 0.0, Color32::from_rgb(9, 9, 12));
            vertical_gradient(
                &painter,
                screen,
                Color32::from_rgb(14, 14, 19),
                Color32::from_rgb(7, 7, 9),
            );
        }

        if self.book.is_none() {
            self.draw_empty(&painter, screen, &mut actions);
        } else {
            self.update_drawer(&ctx, screen);
            // 顶部一条极淡的压暗/提亮，让标题栏和画面之间有过渡
            let scrim_top = if self.is_light() {
                Color32::from_rgba_unmultiplied(250, 250, 252, 200)
            } else {
                Color32::from_rgba_unmultiplied(6, 6, 9, 190)
            };
            let scrim_bot = if self.is_light() {
                Color32::from_rgba_unmultiplied(250, 250, 252, 0)
            } else {
                Color32::from_rgba_unmultiplied(6, 6, 9, 0)
            };
            vertical_gradient(
                &painter,
                Rect::from_min_size(screen.min, Vec2::new(screen.width(), 76.0)),
                scrim_top,
                scrim_bot,
            );
            self.draw_page(&painter, screen, &ctx);
            self.draw_nav(&painter, screen, &ctx, &mut actions);
            self.draw_drawer(&painter, screen, &ctx, &mut actions);
            self.draw_hud(&painter, screen);
        }
        self.draw_recent(&painter, screen, &ctx, &mut actions);
        // 窗口按钮与标题任何状态下都要在（空状态也得能关窗）
        self.draw_titlebar(&painter, screen, &ctx, &mut actions);

        // 空白处右键：在浅色 / 深色之间切换
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let right_click = ctx.input(|i| i.pointer.secondary_clicked());
        if right_click {
            if let Some(q) = pointer {
                let title = Rect::from_min_size(screen.min, Vec2::new(screen.width(), 38.0));
                let bottom = Rect::from_min_max(
                    Pos2::new(screen.left(), screen.bottom() - (self.drawer.max(HANDLE_REST) + 34.0)),
                    screen.max,
                );
                let panel = if self.recent_open {
                    Rect::from_min_max(
                        Pos2::new(screen.left() + 4.0, screen.top() + 54.0),
                        Pos2::new(screen.left() + 232.0, screen.bottom() - 24.0),
                    )
                } else {
                    Rect::NOTHING
                };
                let over_page = self.page_rect.map(|r| r.contains(q)).unwrap_or(false);
                if !title.contains(q) && !bottom.contains(q) && !panel.contains(q) && !over_page {
                    actions.toggle_theme = true;
                }
            }
        }

        actions
    }

    fn handle_input(&mut self, ctx: &egui::Context, actions: &mut Actions) {
        let (keys, scroll) = ctx.input(|i| {
            let mut k = Vec::new();
            for key in [
                egui::Key::ArrowRight,
                egui::Key::ArrowLeft,
                egui::Key::PageDown,
                egui::Key::PageUp,
                egui::Key::Space,
                egui::Key::Home,
                egui::Key::End,
                egui::Key::O,
                egui::Key::Escape,
            ] {
                if i.key_pressed(key) {
                    k.push(key);
                }
            }
            (k, i.smooth_scroll_delta)
        });

        let ctrl = ctx.input(|i| i.modifiers.command);
        for key in keys {
            match key {
                egui::Key::ArrowRight | egui::Key::PageDown => self.next(),
                egui::Key::ArrowLeft | egui::Key::PageUp => self.prev(),
                egui::Key::Space => self.next(),
                egui::Key::Home => self.goto(0),
                egui::Key::End => self.goto(self.total().saturating_sub(1)),
                egui::Key::O if ctrl => actions.open_dialog = true,
                egui::Key::Escape => {
                    if self.book.is_some() {
                        actions.close = false;
                    }
                }
                _ => {}
            }
        }

        if self.book.is_some() && scroll.y.abs() > 1.0 {
            let hover = ctx.input(|i| i.pointer.hover_pos());
            let vp = ctx.input(|i| i.viewport_rect());
            // 指针压在「最近打开」面板上时，滚轮归列表，不翻页
            let over_recent = self.recent_open
                && hover
                    .map(|q| {
                        q.x >= vp.left()
                            && q.x <= vp.left() + 240.0
                            && q.y >= vp.top() + 50.0
                            && q.y <= vp.bottom() - 16.0
                    })
                    .unwrap_or(false);
            let hover_bottom = hover.map(|p| p.y).unwrap_or(0.0);
            let limit = ctx.input(|i| i.viewport_rect()).bottom()
                - (self.drawer.max(HANDLE_REST) + 44.0);
            if !over_recent && hover_bottom < limit {
                if scroll.y < 0.0 {
                    self.next();
                } else {
                    self.prev();
                }
            }
        }
    }

    /// 左右边缘浮动的翻页箭头：鼠标贴边才出现，不挡画面。
    fn draw_nav(
        &self,
        p: &egui::Painter,
        screen: Rect,
        ctx: &egui::Context,
        actions: &mut Actions,
    ) {
        if self.book.is_none() {
            return;
        }
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let cy = screen.center().y - 26.0;
        let page_bottom = screen.bottom() - self.drawer.max(HANDLE_REST) - 40.0;
        let gate = |q: Pos2| q.y > screen.top() + 116.0 && q.y < page_bottom;
        let sides = [
            (false, self.current > 0),
            (true, self.current + 1 < self.total()),
        ];
        for (right_side, enabled) in sides {
            let cx = if right_side {
                screen.right() - 54.0
            } else {
                screen.left() + 54.0
            };
            let rect = Rect::from_center_size(Pos2::new(cx, cy), Vec2::splat(46.0));
            let edge_hot = pointer
                .map(|q| {
                    let near_edge = if right_side {
                        q.x > screen.right() - 92.0
                    } else {
                        q.x < screen.left() + 92.0
                    };
                    near_edge && gate(q)
                })
                .unwrap_or(false);
            let hovered = pointer.map(|q| rect.contains(q)).unwrap_or(false);
            let t = ctx.animate_bool_with_time(
                egui::Id::new(if right_side { "nav-r" } else { "nav-l" }),
                edge_hot && enabled,
                0.14,
            );
            if t <= 0.01 || !enabled {
                continue;
            }
            let a = (t * 255.0) as u8;
            // 圆钮：浅色界面是白底，深色界面是淡淡的玻璃底
            p.circle_filled(
                rect.center(),
                23.0,
                self.plate((a as f32 * 0.82) as u8),
            );
            p.circle_stroke(
                rect.center(),
                23.0,
                Stroke::new(1.0, self.ink((a as f32 * 0.35) as u8)),
            );
            let ink = self.ink((a as f32 * if hovered { 0.92 } else { 0.58 }) as u8);
            let dir = if right_side { 1.0 } else { -1.0 };
            let tip = rect.center() + Vec2::new(7.0 * dir, 0.0);
            let base_x = rect.center().x - 2.5 * dir;
            p.add(egui::Shape::convex_polygon(
                vec![
                    tip,
                    Pos2::new(base_x, rect.center().y - 8.0),
                    Pos2::new(base_x, rect.center().y + 8.0),
                ],
                ink,
                Stroke::NONE,
            ));
            if hovered && ctx.input(|i| i.pointer.primary_clicked()) {
                actions.goto = Some(if right_side {
                    self.current + 1
                } else {
                    self.current.saturating_sub(1)
                });
            }
            if t > 0.0 && t < 1.0 {
                ctx.request_repaint_after(std::time::Duration::from_millis(8));
            }
        }
    }

    fn draw_empty(&self, p: &egui::Painter, screen: Rect, actions: &mut Actions) {
        let center = screen.center() - Vec2::new(0.0, 24.0);
        p.text(
            center - Vec2::new(0.0, 30.0),
            Align2::CENTER_CENTER,
            "comic++",
            FontId::proportional(34.0),
            self.ink(236),
        );
        p.text(
            center + Vec2::new(0.0, 12.0),
            Align2::CENTER_CENTER,
            "把 zip / cbz / 7z 或图片文件夹拖进来",
            FontId::proportional(13.5),
            self.ink(120),
        );
        if !self.status.is_empty() {
            p.text(
                center + Vec2::new(0.0, 44.0),
                Align2::CENTER_CENTER,
                &self.status,
                FontId::proportional(12.5),
                Color32::from_rgb(240, 120, 120),
            );
        }

        // 主按钮
        let btn = Rect::from_center_size(center + Vec2::new(0.0, 92.0), Vec2::new(126.0, 36.0));
        let hovered = p
            .ctx()
            .input(|i| i.pointer.hover_pos().map(|q| btn.contains(q)).unwrap_or(false));
        let t = p.ctx().animate_bool_with_time(egui::Id::new("open-btn"), hovered, 0.15);
        let bg = lerp_color(
            Color32::from_rgb(84, 82, 214),
            Color32::from_rgb(106, 104, 240),
            t,
        );
        p.rect_filled(btn, 10.0, bg);
        p.rect_stroke(
            btn,
            10.0,
            Stroke::new(1.0, Color32::from_white_alpha(28)),
            StrokeKind::Inside,
        );
        p.text(
            btn.center(),
            Align2::CENTER_CENTER,
            "打开漫画",
            FontId::proportional(13.5),
            Color32::from_white_alpha(245),
        );
        p.text(
            btn.center() + Vec2::new(0.0, 40.0),
            Align2::CENTER_CENTER,
            "Ctrl + O",
            FontId::proportional(11.0),
            self.ink(70),
        );
        if hovered && p.ctx().input(|i| i.pointer.primary_clicked()) {
            actions.open_dialog = true;
        }
    }

    /// 左侧"最近打开"：一个时钟按钮，点开后竖排两张封面（图片 / 压缩包首图）。
    fn draw_recent(
        &mut self,
        p: &egui::Painter,
        screen: Rect,
        ctx: &egui::Context,
        actions: &mut Actions,
    ) {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let btn = Rect::from_center_size(
            Pos2::new(screen.left() + 30.0, screen.top() + 92.0),
            Vec2::splat(34.0),
        );
        // 时钟上面一个「+」：直接打开文件
        let plus = Rect::from_center_size(
            Pos2::new(screen.left() + 30.0, screen.top() + 46.0),
            Vec2::splat(30.0),
        );
        let plus_hover = pointer.map(|q| plus.expand(5.0).contains(q)).unwrap_or(false);
        if plus_hover {
            p.circle_filled(plus.center(), 15.0, self.plate(210));
            p.circle_stroke(
                plus.center(),
                15.0,
                Stroke::new(1.0, self.ink(28)),
            );
        }
        let plus_col = self.ink(if plus_hover { 220 } else { 118 });
        p.line_segment(
            [
                plus.center() - Vec2::new(6.0, 0.0),
                plus.center() + Vec2::new(6.0, 0.0),
            ],
            Stroke::new(1.2, plus_col),
        );
        p.line_segment(
            [
                plus.center() - Vec2::new(0.0, 6.0),
                plus.center() + Vec2::new(0.0, 6.0),
            ],
            Stroke::new(1.2, plus_col),
        );
        if plus_hover && ctx.input(|i| i.pointer.primary_clicked()) {
            actions.open_dialog = true;
        }
        let hovered = pointer.map(|q| btn.expand(5.0).contains(q)).unwrap_or(false);
        if self.demo && !self.demo_fired && self.demo_start.elapsed().as_secs_f32() > 0.7 {
            self.recent_open = true;
        }
        if hovered || self.recent_open {
            p.circle_filled(btn.center(), 17.0, self.plate(210));
            p.circle_stroke(
                btn.center(),
                17.0,
                Stroke::new(1.0, self.ink(if self.recent_open { 46 } else { 28 })),
            );
        }
        // 时钟指针
        let col = self.ink(if hovered || self.recent_open { 210 } else { 105 });
        p.circle_stroke(btn.center(), 8.0, Stroke::new(1.2, col));
        p.line_segment(
            [btn.center(), btn.center() + Vec2::new(0.0, -6.0)],
            Stroke::new(1.2, col),
        );
        p.line_segment(
            [btn.center(), btn.center() + Vec2::new(4.2, 2.4)],
            Stroke::new(1.2, col),
        );

        if hovered && ctx.input(|i| i.pointer.primary_clicked()) {
            self.recent_open = !self.recent_open;
            if self.recent_open {
                self.recent_fade = 1.0;
                self.recent_hold = 0.0;
                self.recent_wait = None;
                self.recent_closing = false;
                self.recent_active = None;
            }
        }
        if !self.recent_open {
            // 面板收起来之后才动顺序，开着的时候列表是定住的
            self.promote_pending();
            return;
        }

        // 点条目后不马上收：等右侧新书真的出现，再等 1s，然后 0.22s 渐隐
        let dt = ctx.input(|i| i.stable_dt).clamp(0.001, 0.05);
        if let Some(w) = self.recent_wait.clone() {
            if self.book.as_ref().map(|b| b.path == w).unwrap_or(false) {
                self.recent_wait = None;
                self.recent_hold = 1.0;
            }
        }
        if self.recent_closing {
            ctx.request_repaint();
            self.recent_fade -= dt / 0.22;
            if self.recent_fade <= 0.0 {
                self.recent_fade = 0.0;
                self.recent_closing = false;
                self.recent_open = false;
                self.recent_active = None;
                return;
            }
        } else if self.recent_hold > 0.0 {
            ctx.request_repaint();
            self.recent_hold = (self.recent_hold - dt).max(0.0);
            if self.recent_hold <= 0.0 {
                self.recent_closing = true;
            }
        }

        if self.recent.is_empty() {
            p.text(
                btn.center() + Vec2::new(58.0, 2.0),
                Align2::CENTER_CENTER,
                "打开过的文件会留在这里",
                FontId::proportional(11.0),
                self.ink(80),
            );
            return;
        }

        // 紧凑竖排：一条一行；窗口矮的时候用滚轮看后面的
        let row_w = 210.0;
        let cover_w = 62.0;
        let cover_h = 76.0;
        let row_h = RECENT_ROW_H;
        let list_top = btn.bottom() + 8.0;
        let list_bottom = screen.bottom() - 16.0;
        let content_h = self.recent.len() as f32 * row_h;
        let max_scroll = (content_h - (list_bottom - list_top).max(0.0)).max(0.0);
        let list = Rect::from_min_max(
            Pos2::new(screen.left() + 6.0, list_top),
            Pos2::new(screen.left() + 238.0, list_bottom),
        );
        // 面板一整块都能滚：滚轮不用压在缩略图上，按住还能往下拖
        let panel = Rect::from_min_max(
            Pos2::new(screen.left() + 2.0, btn.bottom() + 2.0),
            Pos2::new(screen.left() + 240.0, list_bottom),
        );
        let inside = pointer.map(|q| panel.contains(q)).unwrap_or(false);
        if inside {
            let delta = ctx.input(|i| i.smooth_scroll_delta.y);
            if delta.abs() > 0.1 {
                self.recent_target -= delta;
            }
        }
        if ctx.input(|i| i.pointer.primary_pressed()) {
            if let Some(q) = pointer.filter(|q| panel.contains(*q)) {
                self.recent_drag = Some((q.y, self.recent_scroll));
                self.recent_dragged = false;
                self.recent_target = self.recent_scroll;
            }
        }
        if let Some((y0, s0)) = self.recent_drag {
            if ctx.input(|i| i.pointer.primary_down()) {
                if let Some(q) = pointer {
                    let dy = q.y - y0;
                    if dy.abs() > 3.0 {
                        self.recent_dragged = true;
                    }
                    self.recent_scroll = s0 - dy;
                    self.recent_target = self.recent_scroll;
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                }
            } else {
                // 松手后目标吸附到最近的一整行
                self.recent_drag = None;
            }
        } else if inside && max_scroll > 0.0 {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
        // 停下来就停在整行上：顶上半截的黑杠不会出现，滚动也是滑过去的
        let target = ((self.recent_target / RECENT_ROW_H).round() * RECENT_ROW_H)
            .clamp(0.0, max_scroll);
        if self.recent_drag.is_none() {
            let dt = ctx.input(|i| i.stable_dt).clamp(0.001, 0.05);
            let k = 1.0 - (-dt * 13.0).exp();
            self.recent_scroll += (target - self.recent_scroll) * k;
            if (target - self.recent_scroll).abs() < 0.4 {
                self.recent_scroll = target;
            } else {
                ctx.request_repaint();
            }
        }
        self.recent_scroll = self.recent_scroll.clamp(0.0, max_scroll);
        self.recent_target = self.recent_target.clamp(0.0, max_scroll);
        let mut pa = p.clone();
        pa.set_opacity(self.recent_fade);
        let p = &pa.with_clip_rect(list);
        let mut y = list_top - self.recent_scroll;
        let mut clicked: Option<PathBuf> = None;
        if self.demo && !self.demo_fired && self.demo_start.elapsed().as_secs_f32() > 1.6 {
            self.demo_fired = true;
            clicked = self.recent.iter().nth(1).map(|i| i.path.clone());
        }
        for item in self.recent.iter() {
            let row = Rect::from_min_size(
                Pos2::new(screen.left() + 12.0, y),
                Vec2::new(row_w, row_h - 6.0),
            );
            let cover = Rect::from_min_size(
                Pos2::new(row.left() + 4.0, row.top() + 4.0),
                Vec2::new(cover_w, cover_h),
            );
            let hot = pointer.map(|q| row.contains(q)).unwrap_or(false);
            let active = self
                .recent_active
                .as_ref()
                .map(|a| *a == item.path)
                .unwrap_or(false);
            if hot || active {
                p.rect_filled(row, 7.0, self.veil(if active { 22 } else { 12 }));
            }
            let aspect = item
                .cover
                .as_ref()
                .map(|t| {
                    let s = t.size_vec2();
                    if s.y > 0.0 {
                        s.x / s.y
                    } else {
                        0.72
                    }
                })
                .unwrap_or(0.72);
            let fit = if item.cover.is_some() {
                fit_in(cover, aspect)
            } else {
                cover
            };
            if let Some(tex) = &item.cover {
                soft_shadow(
                    p,
                    fit,
                    0.0,
                    Vec2::new(0.0, 2.0),
                    10.0,
                    if self.is_light() { 46 } else { 110 },
                );
                rounded_image(
                    p,
                    tex.id(),
                    fit,
                    3.0,
                    Color32::from_white_alpha(if hot { 255 } else if self.is_light() { 255 } else { 224 }),
                );
            } else {
                p.rect_filled(cover, 0.0, self.veil(14));
                p.text(
                    cover.center(),
                    Align2::CENTER_CENTER,
                    "载入…",
                    FontId::proportional(9.5),
                    self.ink(85),
                );
            }
            let current = self.book.as_ref().map(|b| b.path == item.path).unwrap_or(false);
            // 类型 + 文件名放在封面右边，长名字能多露几个字
            let tx = cover.right() + 10.0;
            p.text(
                Pos2::new(tx, row.top() + 18.0),
                Align2::LEFT_CENTER,
                item.kind,
                FontId::proportional(9.5),
                self.ink(125),
            );
            p.text(
                Pos2::new(tx, row.top() + 40.0),
                Align2::LEFT_CENTER,
                short_name(&item.title, 15),
                FontId::proportional(11.0),
                self.ink(if hot || active || current { 230 } else { 165 }),
            );
            if hot && !self.recent_dragged && ctx.input(|i| i.pointer.primary_clicked()) {
                clicked = Some(item.path.clone());
            }
            y += row_h;
        }
        if let Some(path) = clicked {
            let same = self.book.as_ref().map(|b| b.path == path).unwrap_or(false);
            // 不立刻收起：留在这里亮着，等右侧翻完再渐隐
            self.recent_active = Some(path.clone());
            self.recent_fade = 1.0;
            self.recent_closing = false;
            if same {
                self.recent_wait = None;
                self.recent_hold = 1.0;
            } else {
                self.recent_wait = Some(path.clone());
                self.recent_hold = 0.0;
                actions.open_path = Some(path);
            }
        }
    }

    fn draw_page(&mut self, p: &egui::Painter, screen: Rect, ctx: &egui::Context) {
        self.page_rect = None;
        self.page_rect2 = None;
        // 抽屉拉多高，画面底部就让出多少：窗口尺寸不变，画面自己缩
        let bottom_limit = screen.bottom() - self.drawer.max(HANDLE_REST) - 16.0;
        let avail = Vec2::new(
            screen.width() - 128.0,
            (bottom_limit - screen.top() - 52.0).max(60.0),
        );
        let mid_y = screen.top() + 52.0 + avail.y * 0.5;

        // 双页模式：当前页和下一页并排；最后一页只剩一张，就照单页居中
        let two = self.spread && self.total() > 1 && self.current + 1 < self.total();
        if two {
            let a = self.current;
            let b = self.current + 1;
            let tex = |idx: usize| {
                self.full
                    .get(&idx)
                    .map(|t| (t.id(), t.size_vec2()))
                    .filter(|(_, s)| s.x > 1.0 && s.y > 1.0)
            };
            let ta = tex(a);
            let tb = tex(b);
            let Some(base) = ta.map(|t| t.1).or_else(|| tb.map(|t| t.1)) else {
                p.text(
                    screen.center(),
                    Align2::CENTER_CENTER,
                    format!(
                        "第 {} / {} 页 · 载入中…",
                        self.current + 1,
                        self.total()
                    ),
                    FontId::proportional(14.0),
                    self.ink(120),
                );
                return;
            };
            let gap = 6.0;
            let half = ((avail.x - gap) * 0.5).max(40.0);
            // 两张一起缩，同一比例，高度才齐
            let scale = (half / base.x).min(avail.y / base.y).min(24.0);
            let sz = |t: Option<(egui::TextureId, Vec2)>| {
                t.map(|t| t.1 * scale)
                    .unwrap_or(base * scale)
            };
            let sa = sz(ta);
            let sb = sz(tb);

            // 两种摆法：正常 [当前, 下一页] 与对调 [下一页, 当前]
            let arrange = |w1: f32, w2: f32| {
                let total_w = w1 + gap + w2;
                let x0 = screen.center().x - total_w * 0.5;
                (
                    Pos2::new(x0 + w1 * 0.5, mid_y),
                    Pos2::new(x0 + w1 + gap + w2 * 0.5, mid_y),
                )
            };
            let (ca_nat, cb_nat) = arrange(sa.x, sb.x);
            let (cb_sw, ca_sw) = arrange(sb.x, sa.x);

            // 0 = 正常，1 = 对调。拖着的那张跟手走、从上面过去；另一张走得慢、
            // 中途缩一点、往下沉一点让路，两张才不会正好叠死在中间。
            let travel = (ca_sw - ca_nat).x.max(1.0);
            let (t, mover) = self.spread_pose(a);
            let t = t.clamp(0.0, 1.0);
            let po = t.powf(1.8);
            let ea = if mover == a { t } else { po };
            let eb = if mover == b { t } else { po };
            let shrink = 1.0 - 0.15 * (std::f32::consts::PI * t).sin();
            let lift = -8.0 * (std::f32::consts::PI * t).sin();
            let sink = 14.0 * (std::f32::consts::PI * t).sin();

            let ca = ca_nat + (ca_sw - ca_nat) * ea;
            let cb = cb_nat + (cb_sw - cb_nat) * eb;
            let ra_size = if mover == a { sa } else { sa * shrink };
            let rb_size = if mover == b { sb } else { sb * shrink };
            let ca = ca + Vec2::new(0.0, if mover == a { lift } else { sink });
            let cb = cb + Vec2::new(0.0, if mover == b { lift } else { sink });
            let ra = Rect::from_center_size(ca, ra_size);
            let rb = Rect::from_center_size(cb, rb_size);
            self.page_rect = Some(ra);
            self.page_rect2 = Some(rb);

            let shadow = if self.is_light() { 72 } else { 140 };
            if !self.is_light() {
                for r in [ra, rb] {
                    radial_glow(
                        p,
                        r.center(),
                        Vec2::new(r.width() * 0.92, r.height() * 0.78),
                        Color32::from_rgba_unmultiplied(150, 146, 255, 12),
                    );
                }
            }
            // 拖着的那张画在上面，先画另一张
            let order = if mover == a {
                [(b, rb, tb), (a, ra, ta)]
            } else {
                [(a, ra, ta), (b, rb, tb)]
            };
            for (_, r, tx) in order {
                match tx {
                    Some((id, _)) => {
                        soft_shadow(p, r, 2.0, Vec2::new(0.0, 5.0), 28.0, shadow);
                        rounded_image(p, id, r, 3.0, Color32::WHITE);
                    }
                    None => {
                        p.rect_filled(r, 3.0, self.veil(10));
                    }
                }
            }
            // 命中判定必须放在矩形算好之后：早一步 self.page_rect 还是空的，按下去抓不到图
            self.update_spread_drag(ctx, travel);
            return;
        }

        let Some(tex) = self.full.get(&self.current) else {
            p.text(
                screen.center(),
                Align2::CENTER_CENTER,
                format!("第 {} / {} 页 · 载入中…", self.current + 1, self.total()),
                FontId::proportional(14.0),
                self.ink(120),
            );
            return;
        };
        let size = tex.size_vec2();
        if size.x <= 1.0 || size.y <= 1.0 {
            return;
        }
        // 抽屉拉多高，画面底部就让出多少：窗口尺寸不变，画面自己缩
        let scale = (avail.x / size.x).min(avail.y / size.y).min(24.0);
        let draw_size = size * scale;
        let center = Pos2::new(screen.center().x, mid_y);
        let rect = Rect::from_center_size(center, draw_size);
        self.page_rect = Some(rect);

        if !self.is_light() {
            // 页面自己在发光：一圈真正柔和的径向光，像暗房里打着的一束灯
            radial_glow(
                p,
                rect.center(),
                Vec2::new(draw_size.x * 0.92, draw_size.y * 0.78),
                Color32::from_rgba_unmultiplied(150, 146, 255, 12),
            );
        }
        // 直角图直接贴在背景上，只留一点很轻的投影，不再描边
        let shadow = if self.is_light() { 72 } else { 140 };
        soft_shadow(p, rect, 2.0, Vec2::new(0.0, 5.0), 28.0, shadow);
        rounded_image(p, tex.id(), rect, 3.0, Color32::WHITE);
        // 单页没有对调这回事：残留的拖拽状态、以及一路传进来没人接的按下/抬起都清掉，
        // 否则攒到下次进双页会拿一个旧按键当「按下」
        self.spread_drag = None;
        self.raw_press = false;
        self.raw_release = false;
    }

    /// 双页模式下按住一张拖：它跟手走，对面那张缩着让路；松手时拖够了就换过去
    fn update_spread_drag(&mut self, ctx: &egui::Context, travel: f32) {
        // 指针优先用 winit 直接喂进来的那份；--demo-drag 时只用演示脚本那一份，
        // 真正的鼠标点击（egui 自己也会收到）不许插手，否则一轮都跑不干净
        let pointer = self.raw_pos.or_else(|| ctx.input(|i| i.pointer.interact_pos()));
        let (egui_press, egui_release) = if self.demo_drag {
            (false, false)
        } else {
            ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()))
        };
        let pressed = self.raw_press || egui_press;
        let released = self.raw_release || egui_release;
        self.raw_press = false;
        self.raw_release = false;
        if !self.spread {
            self.spread_drag = None;
            if pressed {
                self.drag_log("press but not spread");
            }
            return;
        }

        if pressed {
            if let Some(q) = pointer {
                let a = self.current;
                let b = self.current + 1;
                // 哪张图在哪一侧要看几何，不能靠 spread_t 猜：动画中途两张会换边
                let mid = match (self.page_rect, self.page_rect2) {
                    (Some(r1), Some(r2)) => (r1.center().x + r2.center().x) * 0.5,
                    _ => q.x,
                };
                let on_left = q.x < mid;
                let mut hit: Option<(&'static str, usize)> = None;
                if self.page_rect.map(|r| r.contains(q)).unwrap_or(false) {
                    hit = Some(("a", a));
                } else if self.page_rect2.map(|r| r.contains(q)).unwrap_or(false) {
                    hit = Some(("b", b));
                }
                if let Some((_, idx)) = hit {
                    self.spread_drag = Some(SpreadDrag {
                        idx,
                        start_x: q.x,
                        from_left: on_left,
                        base: self.spread_t,
                        f: 0.0,
                    });
                    self.drag_log(&format!(
                        "press idx={idx} from_left={on_left} base={:.3} q=({:.0},{:.0}) frames={} raw={:?}",
                        self.spread_t, q.x, q.y, self.demo_drag_frames, self.raw_pos
                    ));
                } else {
                    self.drag_log(&format!(
                        "press miss q=({:.0},{:.0}) raw={:?} egui={:?} frames={} rects={:?}/{:?}",
                        q.x,
                        q.y,
                        self.raw_pos,
                        ctx.input(|i| i.pointer.interact_pos()),
                        self.demo_drag_frames,
                        self.page_rect,
                        self.page_rect2
                    ));
                }
            }
        }

        if let Some(d) = self.spread_drag.as_mut() {
            if let Some(q) = pointer {
                let raw = (q.x - d.start_x) / travel.max(1.0);
                // 拉过头就走橡皮筋，别让图飞出窗外
                d.f = if raw.abs() <= 0.30 {
                    raw
                } else {
                    raw.signum() * (0.30 + (raw.abs() - 0.30) * 0.4)
                }
                .clamp(-0.85, 0.85);
            }
        }

        if released {
            let Some(d) = self.spread_drag.take() else { return };
            let (p, to) = spread_release(d.base, d.idx, self.current, d.from_left, d.f);
            self.spread_t = p;
            self.spread_mover = Some(d.idx);
            self.spread_t_to = to;
            self.drag_log(&format!(
                "release idx={} f={:.3} from_left={} t={:.3} to={:.3}",
                d.idx, d.f, d.from_left, p, to
            ));
            ctx.request_repaint();
        }
    }

    /// 拖拽自测时把过程写进 testdata\drag.log，release 进程没有控制台，只能落文件
    fn drag_log(&self, msg: &str) {
        if !self.demo_drag {
            return;
        }
        use std::io::Write;
        let path = state_dir().join("drag.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{msg}");
        }
    }

    /// 当前该用哪个进度绘图：拖拽时把手指位移叠上去，停下就用动画进度
    fn spread_pose(&self, a: usize) -> (f32, usize) {
        if let Some(d) = &self.spread_drag {
            let dir = if d.idx == a { 1.0 } else { -1.0 };
            return ((d.base + dir * d.f).clamp(0.0, 1.0), d.idx);
        }
        (self.spread_t, self.spread_mover.unwrap_or(a))
    }

    /// 让 grabbed 这张滑到另一边去：对面那张缩一点从底下挤过去
    fn start_spread_swap(&mut self, grabbed: usize) {
        self.spread_mover = Some(grabbed);
        self.spread_t_to = if self.spread_t_to > 0.5 { 0.0 } else { 1.0 };
    }

    /// 推进对调动画：指数逼近，接着手指松手的位置继续走，不会有跳帧感
    fn tick_spread_anim(&mut self, ctx: &egui::Context) {
        // 手指正抓着的时候由手指决定位置，动画别插一脚
        if self.spread_drag.is_some() {
            return;
        }
        let d = self.spread_t_to - self.spread_t;
        if d.abs() < 0.002 {
            self.spread_t = self.spread_t_to;
            return;
        }
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0005, 0.05);
        self.spread_t += d * (1.0 - (-dt * 9.0).exp());
        ctx.request_repaint();
    }

    /// `--demo-drag`：走和真鼠标一模一样的通道（set_pointer / pointer_button）连拖三轮，
    /// 左图往右、右图往左、再左图往右，用来验「能不能反复拖」。
    /// 按帧计数，不按时间——窗口被遮住时帧会很稀，时间轴会把整轮压扁。
    fn demo_drag_tick(&mut self, ctx: &egui::Context) {
        if self.demo_drag_at.is_none() {
            self.drag_log("boot demo-drag");
        }
        self.demo_drag_at.get_or_insert_with(|| std::time::Instant::now());
        const PER_ROUND: u32 = 32;
        const ROUNDS: u32 = 3;
        let c = self.demo_drag_frames;
        ctx.request_repaint();
        if c >= PER_ROUND * ROUNDS {
            return;
        }
        let round = (c / PER_ROUND) as usize;
        let phase = c % PER_ROUND;
        let grab_left = round % 2 == 0;
        let key = (round as u8) * 2 + 1;

        if phase == 0 {
            let rects = match (self.page_rect, self.page_rect2) {
                (Some(r1), Some(r2)) => {
                    let (l, r) = if r1.center().x <= r2.center().x {
                        (r1, r2)
                    } else {
                        (r2, r1)
                    };
                    Some((l, r))
                }
                _ => None,
            };
            // 第一帧两页可能还没画出来，这一帧不算数，等下一帧
            let Some((l, r)) = rects else { return };
            // 起手这一帧就老老实实按在原位，别一按下已经偏出去半个身位
            self.demo_drag_step = key;
            self.demo_drag_anchor = if grab_left { l.center() } else { r.center() };
            self.demo_drag_span = (r.center().x - l.center().x).abs().max(120.0);
            self.set_pointer(self.demo_drag_anchor.x, self.demo_drag_anchor.y);
            self.pointer_button(true);
        } else if phase <= 8 {
            // 1..=8 帧推过去，推到 75% 的两槽间距
            let k = phase as f32 / 8.0;
            let dx = if grab_left { 1.0 } else { -1.0 } * self.demo_drag_span * 0.75 * k;
            self.set_pointer(self.demo_drag_anchor.x + dx, self.demo_drag_anchor.y);
        } else if phase == 9 {
            self.demo_drag_step = key + 1;
            self.pointer_button(false);
        }
        self.demo_drag_frames = c + 1;
    }

    /// `--demo-drag` 自测时独占指针通道：真鼠标的事件不再覆盖演示脚本喂进来的位置
    pub fn demo_lock(&self) -> bool {
        self.demo_drag
    }

    /// winit 喂进来的指针位置（逻辑像素，和 egui 同一套坐标）
    pub fn set_pointer(&mut self, x: f32, y: f32) {
        self.raw_pos = Some(Pos2::new(x, y));
    }

    /// 左键按下 / 抬起，给双页拖拽用
    pub fn pointer_button(&mut self, down: bool) {
        if down {
            self.raw_press = true;
        } else {
            self.raw_release = true;
        }
    }

    /// 单页 / 双页、以及漫画的左右阅读朝向（true = 右开、当前页在右边）
    pub fn spread_state(&self) -> (bool, bool) {
        (self.spread, self.spread_t_to > 0.5)
    }

    pub fn set_spread_state(&mut self, spread: bool, swap: bool) {
        self.spread = spread;
        let v = if swap { 1.0 } else { 0.0 };
        self.spread_t = v;
        self.spread_t_to = v;
        self.spread_mover = None;
    }

    fn draw_titlebar(&mut self, p: &egui::Painter, screen: Rect, ctx: &egui::Context, actions: &mut Actions) {
        let bar = Rect::from_min_size(screen.min, Vec2::new(screen.width(), 38.0));
        let pointer = ctx.input(|i| i.pointer.hover_pos());

        // 窗口按钮：macOS 是左侧红绿灯，其它平台是右上角的 Windows 风格三键
        let buttons_rect = if cfg!(target_os = "macos") {
            self.draw_traffic_lights(p, screen, ctx, actions)
        } else {
            self.draw_win_buttons(p, screen, ctx, actions)
        };
        // 单页 / 双页切换，挨在窗口按钮左边
        let mode_rect = self.draw_mode_toggle(p, screen, ctx);

        // 标题：淡淡的居中一行，不抢画面
        let title = if let Some(book) = &self.book {
            if self.spread && self.total() > 1 && self.current + 1 < self.total() {
                let a = self.current + 1;
                let b = self.current + 2;
                format!("{}   {}–{} / {}", book.title, a, b, self.total())
            } else {
                format!("{}   {} / {}", book.title, self.current + 1, self.total())
            }
        } else {
            "comic++".to_string()
        };
        p.text(
            Pos2::new(screen.center().x, bar.center().y + 2.0),
            Align2::CENTER_CENTER,
            title,
            FontId::proportional(12.0),
            self.ink(if self.book.is_some() { 120 } else { 140 }),
        );

        // 拖拽：顶栏空白处按下即拖窗口
        let started = ctx.input(|i| i.pointer.primary_pressed());
        let in_bar = pointer.map(|q| bar.contains(q)).unwrap_or(false);
        let on_buttons = pointer
            .map(|q| {
                buttons_rect.contains(q) || mode_rect.contains(q) || q.y < screen.top() + 6.0
            })
            .unwrap_or(false);
        if in_bar && !on_buttons && started {
            actions.drag = true;
        }
    }

    /// 右上角那个单页/双页的图标：一个竖长小页 = 单页，两个并排 = 双页
    fn draw_mode_toggle(&mut self, p: &egui::Painter, screen: Rect, ctx: &egui::Context) -> Rect {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let bw = 40.0;
        let bh = 30.0;
        let top = screen.top() + 8.0;
        // 在窗口三键左边留 8px；macOS 上按钮在左边，这里就贴着最右
        let right = if cfg!(target_os = "macos") {
            screen.right() - 10.0
        } else {
            screen.right() - 10.0 - 44.0 * 3.0 - 8.0
        };
        let r = Rect::from_min_size(Pos2::new(right - bw, top), Vec2::new(bw, bh));
        let hovered = pointer.map(|q| r.contains(q)).unwrap_or(false);
        let t = ctx.animate_bool_with_time(egui::Id::new("spread_toggle"), hovered, 0.12);
        if t > 0.01 {
            p.rect_filled(r, 7.0, self.veil((t * 26.0) as u8));
        }
        let c = r.center();
        let ink = self.ink(if self.spread { 210 } else { 175 });
        if self.spread {
            for dx in [-6.0f32, 4.0] {
                p.rect_stroke(
                    Rect::from_center_size(c + Vec2::new(dx, 0.0), Vec2::new(7.5, 12.5)),
                    1.5,
                    Stroke::new(1.2, ink),
                    StrokeKind::Inside,
                );
            }
        } else {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::new(9.5, 12.5)),
                1.5,
                Stroke::new(1.2, ink),
                StrokeKind::Inside,
            );
        }
        if hovered && ctx.input(|i| i.pointer.primary_clicked()) {
            self.spread = !self.spread;
            self.spread_drag = None;
            // 切回单页就把对调状态收回去，下次进来还是正常顺序（动画藏着走完，不跳）
            if !self.spread {
                self.spread_t_to = 0.0;
                self.spread_mover = None;
            }
            ctx.request_repaint();
        }
        r
    }

    /// macOS：左侧三个彩色圆点，hover 才露出符号。
    fn draw_traffic_lights(
        &self,
        p: &egui::Painter,
        screen: Rect,
        ctx: &egui::Context,
        actions: &mut Actions,
    ) -> Rect {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let d = 12.0;
        let gap = 8.0;
        let y = screen.top() + 23.0;
        let mut x = screen.left() + 20.0 + d * 0.5;
        let lights = [
            (Color32::from_rgb(255, 95, 87), "×"),
            (Color32::from_rgb(254, 188, 46), "−"),
            (Color32::from_rgb(40, 200, 64), "⤢"),
        ];
        for (i, (color, glyph)) in lights.iter().enumerate() {
            let center = Pos2::new(x, y);
            let hit = Rect::from_center_size(center, Vec2::splat(d + 8.0));
            let hovered = pointer.map(|q| hit.contains(q)).unwrap_or(false);
            p.circle_filled(center, d * 0.5, *color);
            if hovered {
                p.circle_stroke(
                    center,
                    d * 0.5 + 1.0,
                    Stroke::new(1.0, Color32::from_white_alpha(50)),
                );
                p.text(
                    center,
                    Align2::CENTER_CENTER,
                    *glyph,
                    FontId::proportional(8.5),
                    Color32::from_black_alpha(180),
                );
                if ctx.input(|i| i.pointer.primary_clicked()) {
                    match i {
                        0 => actions.close = true,
                        1 => actions.minimize = true,
                        _ => actions.maximize = true,
                    }
                }
            }
            x += d + gap;
        }
        Rect::from_min_size(
            Pos2::new(screen.left(), screen.top()),
            Vec2::new(20.0 + (d + gap) * 3.0, 38.0),
        )
    }

    /// Windows / Linux：右上角三键，hover 才出底色，关闭键 hover 变红。
    fn draw_win_buttons(
        &self,
        p: &egui::Painter,
        screen: Rect,
        ctx: &egui::Context,
        actions: &mut Actions,
    ) -> Rect {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let bw = 44.0;
        let bh = 30.0;
        let top = screen.top() + 8.0;
        let right = screen.right() - 10.0;

        for i in 0..3 {
            let r = Rect::from_min_size(
                Pos2::new(right - bw * (3 - i) as f32, top),
                Vec2::new(bw, bh),
            );
            let hovered = pointer.map(|q| r.contains(q)).unwrap_or(false);
            let t = ctx.animate_bool_with_time(egui::Id::new(("winbtn", i)), hovered, 0.12);
            let is_close = i == 2;
            if t > 0.01 {
                let bg = if is_close {
                    Color32::from_rgba_unmultiplied(232, 17, 35, (t * 240.0) as u8)
                } else {
                    self.veil((t * 26.0) as u8)
                };
                p.rect_filled(r, 7.0, bg);
            }
            let c = r.center();
            let ink = if is_close && hovered {
                Color32::WHITE
            } else {
                self.ink(190)
            };
            match i {
                0 => {
                    p.line_segment([c + Vec2::new(-5.5, 0.0), c + Vec2::new(5.5, 0.0)], Stroke::new(1.0, ink));
                }
                1 => {
                    p.rect_stroke(
                        Rect::from_center_size(c, Vec2::splat(9.5)),
                        1.0,
                        Stroke::new(1.0, ink),
                        StrokeKind::Inside,
                    );
                }
                _ => {
                    p.line_segment([c + Vec2::new(-5.0, -5.0), c + Vec2::new(5.0, 5.0)], Stroke::new(1.0, ink));
                    p.line_segment([c + Vec2::new(5.0, -5.0), c + Vec2::new(-5.0, 5.0)], Stroke::new(1.0, ink));
                }
            }
            if hovered && ctx.input(|i| i.pointer.primary_clicked()) {
                match i {
                    0 => actions.minimize = true,
                    1 => actions.maximize = true,
                    _ => actions.close = true,
                }
            }
        }
        Rect::from_min_size(
            Pos2::new(right - bw * 3.0, top),
            Vec2::new(bw * 3.0, bh),
        )
    }

    /// 白条的拖动逻辑：按下贴着它拖，松手时太矮就弹回去，双击可以一键开/收
    fn update_drawer(&mut self, ctx: &egui::Context, screen: Rect) {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        // 窗口被最小化时高度可能只有几十像素，clamp 的上下界不能倒挂
        let max = (screen.height() - 250.0)
            .min(screen.height() * 0.72)
            .max(0.0);
        let handle_bottom = screen.bottom() - self.drawer.max(HANDLE_REST);
        let hw = (screen.width() * 0.13).clamp(60.0, 150.0);
        let hit = Rect::from_center_size(
            Pos2::new(screen.center().x, handle_bottom - HANDLE_H * 0.5),
            Vec2::new(hw + 80.0, 36.0),
        );
        let in_hit = pointer.map(|q| hit.contains(q)).unwrap_or(false);
        let pressed = ctx.input(|i| i.pointer.primary_pressed());
        let released = ctx.input(|i| i.pointer.primary_released());
        let dbl = ctx.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));

        if in_hit && dbl {
            // 双击：一把推到默认高度，或者直接收回去
            self.drawer_drag = false;
            self.recent_open = false;
            self.drawer_target = if self.drawer > DRAWER_MIN {
                0.0
            } else {
                (max * 0.55).max(DRAWER_MIN + 40.0)
            };
        } else if in_hit && pressed {
            self.drawer_drag = true;
            self.recent_open = false;
            self.drawer_grab = pointer.map(|q| q.y - handle_bottom).unwrap_or(0.0);
        }

        if self.drawer_drag {
            if let Some(q) = pointer {
                let top_min = screen.top() + 120.0;
                let bottom_max = screen.bottom().max(top_min);
                let bottom = (q.y - self.drawer_grab).clamp(top_min, bottom_max);
                self.drawer = (screen.bottom() - bottom).clamp(0.0, max);
                self.drawer_target = self.drawer;
            }
            if released {
                self.drawer_drag = false;
                self.drawer_target = if self.drawer < DRAWER_MIN { 0.0 } else { self.drawer };
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(8));
        } else {
            let dt = ctx.input(|i| i.stable_dt).min(0.05);
            let step = (dt / 0.16).min(1.0);
            self.drawer += (self.drawer_target - self.drawer) * step;
            if (self.drawer_target - self.drawer).abs() < 0.5 {
                self.drawer = self.drawer_target;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(8));
            }
        }
    }

    /// 底部抽屉：白条常驻；往上拉才露出缩略图，窗口高度不变、上面的画面自己缩
    fn draw_drawer(&mut self, p: &egui::Painter, screen: Rect, ctx: &egui::Context, actions: &mut Actions) {
        let total = self.total();
        let handle_bottom = screen.bottom() - self.drawer.max(HANDLE_REST);
        let hw = (screen.width() * 0.13).clamp(60.0, 150.0);
        let handle = Rect::from_center_size(
            Pos2::new(screen.center().x, handle_bottom - HANDLE_H * 0.5),
            Vec2::new(hw, HANDLE_H),
        );
        let over_handle = ctx
            .input(|i| i.pointer.hover_pos())
            .map(|q| Rect::from_center_size(handle.center(), Vec2::new(hw + 56.0, 34.0)).contains(q))
            .unwrap_or(false);
        // 白条：平时 20%，划过或正按着拖的时候 80%；深色界面是白的，浅色界面反过来
        let lit = if over_handle || self.drawer_drag { 204 } else { 51 };
        let bar = if self.is_light() {
            Color32::from_black_alpha(lit)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, lit)
        };
        p.rect_filled(handle, 3.0, bar);

        if total == 0 || self.drawer < DRAWER_MIN {
            return;
        }
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let dt = ctx.input(|i| i.stable_dt).min(0.05);

        // 惯性滚动
        if self.strip_velocity.abs() > 0.5 {
            self.strip_scroll += self.strip_velocity * dt;
            self.strip_velocity *= 0.92;
        }

        // 白条到窗口底的距离有多远，缩略图就有多高、能铺多大的范围
        let rect = Rect::from_min_max(
            Pos2::new(screen.left() + 24.0, handle_bottom + 14.0),
            Pos2::new(screen.right() - 24.0, screen.bottom() - 26.0),
        );
        if rect.height() < 6.0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(8));
            return;
        }
        self.thumb_h = (rect.height() - 18.0).clamp(26.0, 150.0);
        let t = ((self.drawer - DRAWER_MIN) / 70.0).clamp(0.0, 1.0);
        let alpha = (t * 255.0) as u8;

        // 两侧各留 46px 给小箭头；内容裁切区上下多留 30px，投影再大也不会贴到裁切线上
        let strip = Rect::from_min_max(
            Pos2::new(rect.left() + 46.0, rect.top()),
            Pos2::new(rect.right() - 46.0, rect.bottom()),
        );
        let clip = Rect::from_min_max(
            Pos2::new(strip.left() - 22.0, strip.top() - 30.0),
            Pos2::new(strip.right() + 22.0, strip.bottom() + 30.0),
        );
        // 抽屉里不垫任何玻璃/底板，缩略图直接浮在背景上
        {
            // 投影用大一圈的裁切区（不被切边），图本身还是按条子的边界裁，免得压到两侧箭头
            let p_shadow = &p.with_clip_rect(clip);
            let p_img = &p.with_clip_rect(strip);
            let thumb_h = self.thumb_h;
            let gap = THUMB_GAP;
            let mut x = strip.left() - self.strip_scroll;
            let mut hovered_idx = None;
            for idx in 0..total {
                let aspect = self
                    .thumbs
                    .get(&idx)
                    .map(|h| h.size_vec2().x / h.size_vec2().y)
                    .unwrap_or(0.72);
                let w = (thumb_h * aspect).clamp(24.0, 220.0);
                let r0 = Rect::from_min_size(
                    Pos2::new(x, strip.top() + (strip.height() - thumb_h) * 0.5),
                    Vec2::new(w, thumb_h),
                );
                if r0.right() < clip.left() - 10.0 {
                    x += w + gap;
                    continue;
                }
                if r0.left() > clip.right() + 10.0 {
                    break;
                }
                let is_cur = idx == self.current;
                let hovered = pointer
                    .map(|q| strip.contains(q) && r0.contains(q))
                    .unwrap_or(false);
                if hovered {
                    hovered_idx = Some(idx);
                }
                // 当前页整个提起来一点点，比 hover 的 3px 再高一点
                let lift = if is_cur {
                    -5.0
                } else if hovered {
                    -3.0
                } else {
                    0.0
                };
                let r = r0.translate(Vec2::new(0.0, lift));

                if let Some(tex) = self.thumbs.get(&idx) {
                    // 投影交给 epaint 的 Shadow 羽化（一条平滑渐变带），当前页把数值放大一圈
                    let base = if self.is_light() {
                        if is_cur { 92 } else { 26 }
                    } else if is_cur {
                        155
                    } else {
                        56
                    };
                    let a = (base as f32 * alpha as f32 / 255.0) as u8;
                    if a > 0 {
                        soft_shadow(
                            p_shadow,
                            r,
                            if is_cur { 3.0 } else { 0.0 },
                            Vec2::new(0.0, if is_cur { 6.0 } else { 2.0 }),
                            if is_cur { 30.0 } else { 12.0 },
                            a,
                        );
                    }
                    // 当前页给满亮度，其余压暗一点；浅色界面用灰乘而不是透明度，免得灰成一片
                    let tint = if is_cur || hovered {
                        Color32::from_white_alpha(alpha)
                    } else if self.is_light() {
                        let v = (255.0 - (alpha as f32 * 0.16)) as u8;
                        Color32::from_rgb(v, v, v)
                    } else {
                        Color32::from_white_alpha((alpha as f32 * 0.66) as u8)
                    };
                    rounded_image(p_img, tex.id(), r, 3.0, tint);
                } else {
                    p_img.rect_filled(
                        r,
                        0.0,
                        self.veil((alpha as f32 * 0.07) as u8),
                    );
                }

                x += w + gap;
            }
            // hover 只用来判定点击，不再额外画框；点了也不把条子自动滚到别的位置
            if let Some(idx) = hovered_idx {
                if ctx.input(|i| i.pointer.primary_clicked()) {
                    actions.goto = Some(idx);
                }
            }
        }

        // 横向滚动 & 阻尼（只在鼠标落在抽屉里时接管滚轮）
        let in_panel = pointer.map(|q| rect.contains(q)).unwrap_or(false);
        if in_panel {
            let (wheel_v, wheel_h) = ctx.input(|i| (i.smooth_scroll_delta.y, i.smooth_scroll_delta.x));
            let delta = if wheel_h.abs() > 1.0 { wheel_h } else { wheel_v };
            if delta.abs() > 1.0 {
                self.strip_scroll -= delta * 1.2;
                self.strip_velocity = 0.0;
                self.strip_target = None;
            }
        }
        let max_scroll = (self.content_width(total, self.thumb_h) - strip.width()).max(0.0);

        // 刚打开一本书时把当前页的缩略图滚到中间一次
        if self.strip_center_once {
            let (cx, cw) = self.span_at(self.current, self.thumb_h);
            self.strip_scroll =
                (cx + cw * 0.5 - strip.width() * 0.5).clamp(0.0, max_scroll);
            self.strip_center_once = false;
        }

        // 抽屉两侧的小箭头：手动翻一屏缩略图，不跟着页码自动跳
        let step = (strip.width() * 0.86).max(120.0);
        for right_side in [false, true] {
            let enabled = if right_side {
                self.strip_scroll < max_scroll - 2.0
            } else {
                self.strip_scroll > 2.0
            };
            if !enabled {
                continue;
            }
            let center = Pos2::new(
                if right_side {
                    rect.right() - 16.0
                } else {
                    rect.left() + 16.0
                },
                rect.center().y,
            );
            let hit = Rect::from_center_size(center, Vec2::splat(30.0));
            let hovered = pointer.map(|q| hit.contains(q)).unwrap_or(false);
            let fade = alpha as f32 / 255.0;
            p.circle_filled(
                center,
                14.0,
                self.plate(((if hovered { 205.0 } else { 115.0 }) * fade) as u8),
            );
            let ink = self.ink(((if hovered { 225.0 } else { 145.0 }) * fade) as u8);
            let dir = if right_side { 1.0 } else { -1.0 };
            let tip = center + Vec2::new(5.0 * dir, 0.0);
            p.line_segment(
                [center + Vec2::new(-4.0 * dir, -7.0), tip],
                Stroke::new(1.4, ink),
            );
            p.line_segment(
                [tip, center + Vec2::new(-4.0 * dir, 7.0)],
                Stroke::new(1.4, ink),
            );
            if hovered && ctx.input(|i| i.pointer.primary_clicked()) {
                let target = self.strip_scroll + if right_side { step } else { -step };
                self.strip_target = Some(target.clamp(0.0, max_scroll));
            }
        }

        // 目标位置用缓动逼近，别一格一格地跳
        if let Some(target) = self.strip_target {
            let d = target - self.strip_scroll;
            if d.abs() < 1.0 {
                self.strip_scroll = target;
                self.strip_target = None;
            } else {
                self.strip_scroll += d * (dt * 9.0).min(1.0);
                ctx.request_repaint_after(std::time::Duration::from_millis(8));
            }
        }
        self.strip_scroll = self.strip_scroll.clamp(-8.0, max_scroll + 8.0);
    }

    /// 第 idx 张缩略图在不滚动时的 x 位置和宽度
    fn span_at(&self, idx: usize, thumb_h: f32) -> (f32, f32) {
        let mut x = 0.0;
        for i in 0..=idx {
            let aspect = self
                .thumbs
                .get(&i)
                .map(|h| h.size_vec2().x / h.size_vec2().y)
                .unwrap_or(0.72);
            let w = (thumb_h * aspect).clamp(24.0, 220.0);
            if i == idx {
                return (x, w);
            }
            x += w + THUMB_GAP;
        }
        (0.0, thumb_h * 0.72)
    }

    fn content_width(&self, total: usize, thumb_h: f32) -> f32 {
        let mut w = 0.0;
        for idx in 0..total {
            let aspect = self
                .thumbs
                .get(&idx)
                .map(|h| h.size_vec2().x / h.size_vec2().y)
                .unwrap_or(0.72);
            w += (thumb_h * aspect).clamp(24.0, 220.0) + THUMB_GAP;
        }
        w
    }

    fn draw_hud(&self, p: &egui::Painter, screen: Rect) {
        if self.book.is_none() {
            return;
        }
        // 顶部一条 2px 的细进度：轨道贴着窗口上沿，翻页进度一目了然又不吵
        let frac = if self.total() > 1 {
            self.current as f32 / (self.total() - 1) as f32
        } else {
            0.0
        };
        let track = Rect::from_min_size(
            Pos2::new(screen.left(), screen.top()),
            Vec2::new(screen.width(), 2.0),
        );
        p.rect_filled(track, 0.0, self.veil(8));
        if frac > 0.0 {
            p.rect_filled(
                Rect::from_min_size(track.min, Vec2::new(track.width() * frac, 2.0)),
                0.0,
                Color32::from_rgba_unmultiplied(148, 144, 255, 110),
            );
        }
    }
}

/// 供主循环复用的一次性 helper：把 mesh 变成竖直渐变矩形用的四个顶点。
#[allow(dead_code)]
fn vertical_gradient(p: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    p.add(egui::Shape::mesh(mesh));
}

#[allow(dead_code)]
fn corner(r: f32) -> CornerRadius {
    CornerRadius::same(r as u8)
}

/// 椭圆径向渐变：中心浓、边缘归零，用三角扇 + 多环拼出来。
/// egui 没有原生径向渐变，直接喂几个矩形会看到生硬的方框边界。
fn radial_glow(p: &egui::Painter, center: Pos2, radius: Vec2, color: Color32) {
    const SECTORS: usize = 48;
    const RINGS: usize = 9;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, color);
    for i in 1..=RINGS {
        let t = i as f32 / RINGS as f32;
        let a = (color.a() as f32 * (1.0 - t).powf(2.2)) as u8;
        for k in 0..SECTORS {
            let ang = k as f32 / SECTORS as f32 * std::f32::consts::TAU;
            let pos = center
                + Vec2::new(ang.cos() * radius.x * t, ang.sin() * radius.y * t);
            mesh.colored_vertex(
                pos,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a),
            );
        }
    }
    for k in 0..SECTORS {
        let k2 = (k + 1) % SECTORS;
        mesh.add_triangle(0, 1 + k as u32, 1 + k2 as u32);
    }
    for i in 1..RINGS {
        let base = 1 + (i - 1) as u32 * SECTORS as u32;
        let next = 1 + i as u32 * SECTORS as u32;
        for k in 0..SECTORS {
            let k2 = (k + 1) % SECTORS;
            let (a0, a1) = (base + k as u32, base + k2 as u32);
            let (b0, b1) = (next + k as u32, next + k2 as u32);
            mesh.add_triangle(a0, b0, a1);
            mesh.add_triangle(a1, b0, b1);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Windows 的 canonicalize 会加 `\\?\` 前缀，写进最近记录里很难看，顺手去掉。
fn norm_path(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    p
}

/// 带圆角的图：三角扇 + 圆角边界，UV 按位置映射，3px 圆角不用去改纹理
fn rounded_image(p: &egui::Painter, id: egui::TextureId, rect: Rect, radius: f32, tint: Color32) {
    let r = radius
        .max(0.0)
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5);
    if r < 0.5 {
        p.image(id, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
        return;
    }
    let mut mesh = Mesh::default();
    mesh.texture_id = id;
    let uv = |q: Pos2| {
        Pos2::new(
            ((q.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
            ((q.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
        )
    };
    // 边界点连同外法线一起收，方便在边上再铺一圈全透明的羽化带，
    // 否则 3px 这种小圆角会和背景撞出锯齿。
    let mut pts: Vec<(Pos2, Vec2)> = Vec::new();
    let seg = 10;
    fn arc(out: &mut Vec<(Pos2, Vec2)>, c: Pos2, r: f32, a0: f32, a1: f32, seg: usize) {
        for i in 0..=seg {
            let t = i as f32 / seg as f32;
            let a = (a0 + (a1 - a0) * t).to_radians();
            let nrm = Vec2::new(a.cos(), a.sin());
            out.push((c + nrm * r, nrm));
        }
    }
    arc(&mut pts, Pos2::new(rect.left() + r, rect.top() + r), r, 180.0, 270.0, seg);
    pts.push((Pos2::new(rect.right() - r, rect.top()), Vec2::new(0.0, -1.0)));
    arc(&mut pts, Pos2::new(rect.right() - r, rect.top() + r), r, 270.0, 360.0, seg);
    pts.push((Pos2::new(rect.right(), rect.bottom() - r), Vec2::new(1.0, 0.0)));
    arc(&mut pts, Pos2::new(rect.right() - r, rect.bottom() - r), r, 0.0, 90.0, seg);
    pts.push((Pos2::new(rect.left() + r, rect.bottom()), Vec2::new(0.0, 1.0)));
    arc(&mut pts, Pos2::new(rect.left() + r, rect.bottom() - r), r, 90.0, 180.0, seg);
    pts.push((Pos2::new(rect.left(), rect.top() + r), Vec2::new(-1.0, 0.0)));

    let feather = 1.0;
    let n = pts.len() as u32;
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.center(),
        uv: uv(rect.center()),
        color: tint,
    });
    for (q, _) in &pts {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *q,
            uv: uv(*q),
            color: tint,
        });
    }
    for (q, nrm) in &pts {
        let o = *q + *nrm * feather;
        mesh.vertices.push(egui::epaint::Vertex {
            pos: o,
            uv: uv(o),
            color: Color32::TRANSPARENT,
        });
    }
    for i in 0..n {
        let j = 1 + (i + 1) % n;
        mesh.add_triangle(0, 1 + i, j);
        let oi = 1 + n + i;
        let oj = 1 + n + (i + 1) % n;
        mesh.add_triangle(1 + i, j, oj);
        mesh.add_triangle(1 + i, oj, oi);
    }
    p.add(egui::Shape::mesh(mesh));
}

fn recent_store_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("comicpp").join("recent.txt"))
}

fn is_archive(p: &std::path::Path) -> bool {
    matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "zip" | "cbz" | "cbr" | "7z" | "cb7"
    )
}

/// 通用投影：size=外扩尺寸 / distance=偏移距离 / blur=模糊程度 / opacity=不透明度
fn soft_shadow(p: &egui::Painter, rect: Rect, size: f32, distance: Vec2, blur: f32, opacity: u8) {
    if opacity == 0 {
        return;
    }
    let shadow = egui::epaint::Shadow {
        offset: [distance.x.round() as i8, distance.y.round() as i8],
        blur: blur.round().clamp(0.0, 255.0) as u8,
        spread: size.round().clamp(0.0, 255.0) as u8,
        color: Color32::from_black_alpha(opacity),
    };
    p.add(egui::Shape::Rect(shadow.as_shape(rect, 3.0)));
}

/// 在框内按比例放一张图（contain）
fn fit_in(outer: Rect, aspect: f32) -> Rect {
    let aspect = aspect.max(0.05);
    let mut w = outer.width();
    let mut h = w / aspect;
    if h > outer.height() {
        h = outer.height();
        w = h * aspect;
    }
    Rect::from_center_size(outer.center(), Vec2::new(w, h))
}

fn short_name(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

/// 封面的磁盘缓存：同一份文件（路径 + 修改时间 + 大小）只解一次。
/// 7z 那种首开要解包的，第二次启动直接读缓存小图，几乎瞬间就出来。
fn cover_cache_path(path: &std::path::Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    // 开头这个版本号用来作废旧算法留下的缓存：以前单图取的是「目录第一张」，封面全是错的
    let key = format!("3|{}|{}|{}", path.display(), mtime, meta.len());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    Some(
        base.join("comicpp")
            .join("covers")
            .join(format!("{h:016x}.png")),
    )
}

/// 缓存目录里留最近 100 张，超了就把最老的删掉，免得越积越多。
fn prune_cover_cache(keep: &std::path::Path) {
    let Some(dir) = keep.parent() else { return };
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "png").unwrap_or(false))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= 100 {
        return;
    }
    files.sort_by_key(|f| f.0);
    let drop = files.len() - 100;
    for (_, p) in files.into_iter().take(drop) {
        let _ = std::fs::remove_file(p);
    }
}

/// 后台解一张封面：压缩包取第一页，图片就是它自己
fn spawn_cover(tx: crossbeam_channel::Sender<CoverArt>, path: PathBuf) {
    std::thread::Builder::new()
        .name("cover".into())
        .spawn(move || {
            let cache = cover_cache_path(&path);
            // 命中缓存：只读一张几百像素的小图
            if let Some(cp) = &cache {
                if cp.exists() {
                    if let Ok(img) = image::open(cp) {
                        let img = img.to_rgba8();
                        let (w, h) = (img.width(), img.height());
                        if w > 0 && h > 0 {
                            let _ = tx.send(CoverArt {
                                path,
                                size: [w, h],
                                rgba: img.into_raw(),
                            });
                            return;
                        }
                    }
                }
            }
            let Ok(book) = Book::open(&path) else { return };
            // 单张图打开时 pages 是整个目录的序列，封面必须取「点开的这一张」而不是第 0 张
            let key = path.to_string_lossy().to_string();
            let idx = book.pages.iter().position(|p| p == &key).unwrap_or(0);
            let Ok(mut reader) = crate::book::Reader::open(&book) else {
                return;
            };
            let Ok(raw) = reader.read(idx) else { return };
            let Ok(img) = crate::book::decode_bytes(&raw.bytes) else {
                return;
            };
            let (w, h) = (img.width(), img.height());
            if w == 0 || h == 0 {
                return;
            }
            let tw = w.min(240).max(1);
            let th = ((tw as f32 * h as f32 / w as f32).round() as u32).max(1);
            let small = image::imageops::thumbnail(&img, tw, th);
            if let Some(cp) = &cache {
                if let Some(dir) = cp.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = small.save(cp);
                prune_cover_cache(cp);
            }
            let _ = tx.send(CoverArt {
                path,
                size: [small.width(), small.height()],
                rgba: small.into_raw(),
            });
        })
        .ok();
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), mix(a.a(), b.a()))
}

/// 数据目录：%LOCALAPPDATA%\comicpp（和 main.rs 里的 state.txt 同一个地方）
fn state_dir() -> PathBuf {
    match std::env::var("LOCALAPPDATA") {
        Ok(p) => PathBuf::from(p).join("comicpp"),
        Err(_) => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::spread_release;
    use super::{Pos2, Rect, Vec2, Viewer};

    fn band(left: f32) -> (Rect, Rect) {
        // 左图、右图两个槽位（命中判定用的就是这两个矩形）
        (
            Rect::from_min_size(Pos2::new(left, 52.0), Vec2::new(400.0, 970.0)),
            Rect::from_min_size(Pos2::new(left + 406.0, 52.0), Vec2::new(400.0, 970.0)),
        )
    }

    /// 走一遍真正的按下 / 移动 / 松手（含命中判定），连做三轮，验证能反复对调
    #[test]
    fn drag_sequence_repeats_end_to_end() {
        let ctx = egui::Context::default();
        let mut v = Viewer::new();
        v.spread = true;
        v.current = 0;
        v.spread_t = 0.0;
        v.spread_t_to = 0.0;

        // 只喂 winit 那一路指针（和真机同一通道），egui 自己的 pointer 一直是空的
        let step = |v: &mut Viewer, ctx: &egui::Context| v.update_spread_drag(ctx, 406.0);

        // —— 第一轮：t=0，左槽是 a(当前页)，按住往右拖
        let (l, r) = band(120.0);
        v.page_rect = Some(l);
        v.page_rect2 = Some(r);
        v.set_pointer(l.center().x, l.center().y);
        v.pointer_button(true);
        step(&mut v, &ctx);
        let d = v.spread_drag.as_ref().expect("按下左图就应该抓住它");
        assert_eq!((d.idx, d.from_left), (0, true));

        v.set_pointer(r.center().x, l.center().y);
        step(&mut v, &ctx);
        v.pointer_button(false);
        step(&mut v, &ctx);
        assert_eq!(v.spread_t_to, 1.0, "第一轮应该换过去");
        v.spread_t = 1.0;

        // —— 第二轮：t=1，a 已经挪到右槽，按住右图往左拖，应该换回来
        let (l, r) = band(120.0);
        v.page_rect = Some(r); // page_rect 一直是 a 的矩形，对调后它在右边
        v.page_rect2 = Some(l);
        v.set_pointer(r.center().x, r.center().y);
        v.pointer_button(true);
        step(&mut v, &ctx);
        let d = v.spread_drag.as_ref().expect("第二轮回拖也要抓得住");
        assert_eq!((d.idx, d.from_left), (0, false), "右槽这时是当前页 a");

        v.set_pointer(l.center().x, r.center().y);
        step(&mut v, &ctx);
        v.pointer_button(false);
        step(&mut v, &ctx);
        assert_eq!(v.spread_t_to, 0.0, "第二轮应该换回来");
        v.spread_t = 0.0;

        // —— 第三轮：还是原来的顺序，再按住左图往右拖，照样能换
        let (l, r) = band(120.0);
        v.page_rect = Some(l);
        v.page_rect2 = Some(r);
        v.set_pointer(l.center().x, l.center().y);
        v.pointer_button(true);
        step(&mut v, &ctx);
        assert_eq!(v.spread_drag.as_ref().map(|d| d.idx), Some(0), "第三轮还要抓得住");
        v.set_pointer(r.center().x, l.center().y);
        step(&mut v, &ctx);
        v.pointer_button(false);
        step(&mut v, &ctx);
        assert_eq!(v.spread_t_to, 1.0, "第三轮也换得过去");

        // —— 按右图（b）往左拖，同样是对调
        v.spread_t = 0.0;
        v.spread_t_to = 0.0;
        v.page_rect = Some(l);
        v.page_rect2 = Some(r);
        v.set_pointer(r.center().x, r.center().y);
        v.pointer_button(true);
        step(&mut v, &ctx);
        let d = v.spread_drag.as_ref().expect("右图也该抓得住");
        assert_eq!((d.idx, d.from_left), (1, false));
        v.set_pointer(l.center().x, r.center().y);
        step(&mut v, &ctx);
        v.pointer_button(false);
        step(&mut v, &ctx);
        assert_eq!(v.spread_t_to, 1.0, "抓右图往左拖也是换");
    }

    /// 连拖三次：左图往右 → 右图往左 → 再左图往右，进度必须 0/1/0/1 来回，
    /// 就是用户说的「能反复拖」。
    #[test]
    fn spread_swap_repeats() {
        let cur = 0usize;
        // 1) 正常顺序（t=0）按左边那张（a），往右拖 50%
        let (p1, t1) = spread_release(0.0, 0, cur, true, 0.5);
        assert!(p1 > 0.4, "起手后进度应该跟手：{p1}");
        assert_eq!(t1, 1.0, "拖够了就该对调");

        // 2) 对调后（t=1）按右边那张（此时右边是 a），往左拖 50%
        let (p2, t2) = spread_release(1.0, 0, cur, false, -0.5);
        assert!(p2 < 0.6, "第二轮回拖要跟手：{p2}");
        assert_eq!(t2, 0.0, "第二轮应该换回来");

        // 3) 再按左边那张往右拖
        let (_, t3) = spread_release(0.0, 0, cur, true, 0.5);
        assert_eq!(t3, 1.0, "第三次仍然要能换");

        // 4) 对调后按左边那张（此时左边是 b），往右拖
        let (_, t4) = spread_release(1.0, 1, cur, true, 0.5);
        assert_eq!(t4, 0.0, "抓另一张往右拖同样换回来");
    }

    /// 拖得不够、或者方向不对，都得弹回原位，不能误判成对调
    #[test]
    fn spread_short_drag_snaps_back() {
        let cur = 0usize;
        assert_eq!(spread_release(0.0, 0, cur, true, 0.10).1, 0.0);
        assert_eq!(spread_release(0.0, 0, cur, true, -0.40).1, 0.0);
        assert_eq!(spread_release(1.0, 0, cur, false, 0.40).1, 1.0);
    }
}

