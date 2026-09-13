use crate::{
    assets::Assets,
    markdown::{self, Draw, Layout},
    model::{self, Data},
    theme::Theme,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use zsui::surface::{RasterSurface, SurfacePainter};
use zsui::{
    Dp, Dpi, Rect, UiInvalidationHandle, ViewNode, WidgetId, ZsCanvasPointerEvent,
    ZsCanvasPointerPhase, ZsCanvasPrimitive, ZsCanvasRect, ZsCanvasScene,
};
type BoxRect = [f32; 4];
fn boxrect(b: BoxRect) -> ZsCanvasRect {
    ZsCanvasRect::new(Dp(b[0]), Dp(b[1]), Dp(b[2]), Dp(b[3]))
}
fn contains(b: BoxRect, x: f32, y: f32) -> bool {
    x >= b[0] && y >= b[1] && x < b[0] + b[2] && y < b[1] + b[3]
}
#[derive(Clone, Debug)]
enum Action {
    Collapse,
    Group(String),
    Session(String),
    More,
    Refresh,
    Tab(usize),
    Theme,
    Search,
    Expand(String),
    Copy(String),
    Open(String),
    New,
    Model,
    Tasks,
}
#[derive(Clone)]
enum Msg {
    Pointer(ZsCanvasPointerEvent),
    Scroll(f32),
    SidebarScroll(f32),
    Search(String),
}
#[derive(Clone)]
struct State {
    data: Data,
    base: String,
    dark: bool,
    collapsed: bool,
    groups: HashSet<String>,
    expanded: HashSet<String>,
    tab: usize,
    scroll: f32,
    side_scroll: f32,
    search: Option<String>,
    draft: String,
    loading: bool,
    status: String,
    generation: u64,
    offline: bool,
    tasks_open: bool,
}
struct Renderer {
    p: SurfacePainter,
    assets: Assets,
    hits: Vec<(BoxRect, Action)>,
    layouts: HashMap<String, Layout>,
    revision: u64,
    total: f32,
    side_total: f32,
}
impl Default for Renderer {
    fn default() -> Self {
        Self {
            p: SurfacePainter::default(),
            assets: Assets::default(),
            hits: Vec::new(),
            layouts: HashMap::new(),
            revision: 0,
            total: 0.,
            side_total: 0.,
        }
    }
}
struct App {
    state: Arc<Mutex<State>>,
    renderer: Arc<Mutex<Renderer>>,
    wake: UiInvalidationHandle,
    host: Arc<Mutex<Option<crate::host::OwnedHost>>>,
    capture_only: bool,
}
#[derive(Clone, Copy)]
struct Geometry {
    w: f32,
    h: f32,
    side: f32,
    content: f32,
    x: f32,
    composer: BoxRect,
    timeline: BoxRect,
}
impl Geometry {
    fn new(w: f32, h: f32, collapsed: bool) -> Self {
        let side = if collapsed { 56. } else { 280. };
        let content = (w - side - 48.).clamp(260., 700.);
        let x = side + (w - side - content) / 2.;
        let composer = [x, h - 148., content, 100.];
        let timeline = [side, 76., w - side, (h - 76. - 168.).max(60.)];
        Self {
            w,
            h,
            side,
            content,
            x,
            composer,
            timeline,
        }
    }
}
impl Renderer {
    fn label(
        &mut self,
        r: &mut RasterSurface,
        value: &str,
        b: BoxRect,
        size: f32,
        color: zsui::Color,
        bold: bool,
        t: Theme,
    ) {
        let mut s = t.text(size, color, bold, false);
        s.wrap = zsui::TextWrap::NoWrap;
        s.ellipsis = true;
        self.p.text(r, value, &s, b[0], b[1], b[2], b);
    }
    fn icon(
        &mut self,
        r: &mut RasterSurface,
        id: &str,
        x: f32,
        y: f32,
        size: f32,
        color: zsui::Color,
        t: Theme,
    ) {
        match self.assets.vector(id, size, r.scale, color, t.dark) {
            Ok(f) => r.image(&f, x, y),
            Err(e) => eprintln!("{e}"),
        }
    }
    fn hit(&mut self, b: BoxRect, a: Action) {
        self.hits.push((b, a));
    }
    fn scene(frame: zsui::ZsImageFrame, y: f32, w: f32, h: f32) -> ZsCanvasScene {
        ZsCanvasScene::new().with(ZsCanvasPrimitive::image(frame, boxrect([0., y, w, h])))
    }
    fn build(&mut self, s: &State, bounds: Rect, dpi: Dpi) -> Vec<(ZsCanvasRect, ViewNode<Msg>)> {
        let scale = dpi.scale_factor();
        let mut g = Geometry::new(
            bounds.width as f32 / scale,
            bounds.height as f32 / scale,
            s.collapsed,
        );
        let t = Theme::new(s.dark);
        let todos: Vec<Value> = s
            .data
            .sessions
            .iter()
            .find(|r| r.id == s.data.selected)
            .and_then(|r| r.values.get("todos"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let task_height = if todos.is_empty() {
            0.
        } else {
            42. + if s.tasks_open {
                todos.len().min(4) as f32 * 28. + 8.
            } else {
                0.
            }
        };
        g.timeline[3] = (g.timeline[3] - task_height).max(30.);
        self.hits.clear();
        self.revision += 1;
        let mut background =
            RasterSurface::new(g.w, g.h, scale, t.base).expect("window within surface limit");
        background.rect([0., 0., g.side, g.h], 0., t.sidebar);
        background.rect([g.side - 1., 0., 1., g.h], 0., t.border);
        if g.side > 60. {
            self.icon(&mut background, "BrandWordmark", 16., 24., 24., t.ink, t);
        } else {
            self.icon(&mut background, "FishLogo", 16., 26., 20., t.ink, t);
        }
        let toggle_y = if g.side > 60. { 28. } else { 132. };
        self.icon(
            &mut background,
            "IconPanelLeftOutline16",
            g.side - 34.,
            toggle_y,
            16.,
            t.secondary,
            t,
        );
        self.hit([g.side - 40., toggle_y - 8., 32., 32.], Action::Collapse);
        background.rect([14., 74., g.side - 28., 38.], 10., t.border);
        background.rect([15., 75., g.side - 30., 36.], 9., t.base);
        let nx = if g.side > 60. { g.side / 2. - 44. } else { 20. };
        self.icon(
            &mut background,
            "IconNewChatOutline16",
            nx,
            85.,
            16.,
            t.ink,
            t,
        );
        if g.side > 60. {
            self.label(
                &mut background,
                "新会话",
                [nx + 22., 81., 80., 24.],
                14.,
                t.ink,
                false,
                t,
            );
        }
        self.hit([14., 74., g.side - 28., 38.], Action::New);
        if g.side > 60. {
            self.label(
                &mut background,
                "工作区",
                [16., 130., 120., 24.],
                14.,
                t.muted,
                false,
                t,
            );
            for (id, x, a) in [
                ("IconSearchOutline16", g.side - 98., Action::Search),
                ("IconRefreshOutline16", g.side - 66., Action::Refresh),
                (
                    "IconProjectAddOutline16",
                    g.side - 34.,
                    Action::Open(s.base.clone()),
                ),
            ] {
                self.icon(&mut background, id, x, 134., 16., t.secondary, t);
                self.hit([x - 6., 128., 28., 28.], a);
            }
        }
        self.icon(
            &mut background,
            if s.dark {
                "IconLightOutline16"
            } else {
                "IconDarkOutline16"
            },
            18.,
            g.h - 69.,
            16.,
            t.secondary,
            t,
        );
        if g.side > 60. {
            self.label(
                &mut background,
                if s.dark {
                    "浅色外观"
                } else {
                    "深色外观"
                },
                [42., g.h - 73., 180., 24.],
                13.,
                t.secondary,
                false,
                t,
            );
        }
        self.hit([10., g.h - 78., g.side - 20., 32.], Action::Theme);
        self.icon(
            &mut background,
            "IconSettingsOutline16",
            18.,
            g.h - 34.,
            16.,
            t.secondary,
            t,
        );
        if g.side > 60. {
            self.label(
                &mut background,
                "设置",
                [42., g.h - 38., 180., 24.],
                14.,
                t.ink,
                false,
                t,
            );
        }
        self.hit(
            [10., g.h - 44., g.side - 20., 34.],
            Action::Open(s.base.clone()),
        );
        let session = s.data.sessions.iter().find(|r| r.id == s.data.selected);
        let title = session.map(|r| r.title.as_str()).unwrap_or("新会话");
        let title_w = self
            .p
            .width(title, &t.text(14., t.ink, false, false), scale)
            .min((g.w - g.side - 240.).clamp(90., 370.));
        self.label(
            &mut background,
            title,
            [g.side + 28., 17., title_w, 26.],
            14.,
            t.ink,
            false,
            t,
        );
        if let Some(session) = session {
            let preset = &session.preset;
            let name = if preset.is_empty() || preset == "standard" {
                "标准模式"
            } else {
                preset.as_str()
            };
            self.icon(
                &mut background,
                "IconAgentPresetOutline16",
                g.side + title_w + 42.,
                22.,
                14.,
                t.muted,
                t,
            );
            self.label(
                &mut background,
                name,
                [g.side + title_w + 62., 19., 110., 22.],
                12.,
                t.secondary,
                false,
                t,
            );
        }
        self.icon(
            &mut background,
            "IconRefreshOutline16",
            g.w - 80.,
            22.,
            16.,
            t.secondary,
            t,
        );
        self.hit([g.w - 90., 14., 36., 30.], Action::Refresh);
        self.icon(
            &mut background,
            "IconPanelLeftOutline16",
            g.w - 40.,
            22.,
            16.,
            t.secondary,
            t,
        );
        let mut x = g.side + 28.;
        for (i, name) in ["对话", "轨迹", "产物", "代码图谱", "上下文"]
            .iter()
            .enumerate()
        {
            let w = if i == 3 { 72. } else { 48. };
            self.label(
                &mut background,
                name,
                [x, 48., w, 24.],
                13.,
                if s.tab == i { t.accent } else { t.muted },
                false,
                t,
            );
            if s.tab == i {
                background.rect([x, 73., 26., 2.], 0., t.accent);
            }
            self.hit([x - 4., 43., w, 32.], Action::Tab(i));
            x += w + 14.;
        }
        background.rect([g.side, 75., g.w - g.side, 1.], 0., t.border);
        let [cx, cy, cw, ch] = g.composer;
        if !todos.is_empty() {
            let ty = cy - task_height - 6.;
            background.rect([cx + 8., ty, cw - 16., task_height - 2.], 10., t.card);
            self.icon(
                &mut background,
                "IconChecklistOutline14",
                cx + 20.,
                ty + 12.,
                14.,
                t.secondary,
                t,
            );
            let active = todos
                .iter()
                .filter(|v| v["status"] == "in_progress")
                .count();
            let pending = todos.iter().filter(|v| v["status"] == "pending").count();
            self.label(
                &mut background,
                &format!("任务   {active} 进行中 · {pending} 待处理"),
                [cx + 42., ty + 8., cw - 100., 25.],
                13.,
                t.secondary,
                false,
                t,
            );
            self.icon(
                &mut background,
                if s.tasks_open {
                    "IconChevronUpOutline14"
                } else {
                    "IconChevronDownOutline14"
                },
                cx + cw - 34.,
                ty + 13.,
                12.,
                t.muted,
                t,
            );
            self.hit([cx + 8., ty, cw - 16., 36.], Action::Tasks);
            if s.tasks_open {
                for (i, item) in todos.iter().take(4).enumerate() {
                    let y = ty + 38. + i as f32 * 28.;
                    let done = item["status"] == "completed";
                    self.icon(
                        &mut background,
                        if done {
                            "IconCheckOutline14"
                        } else {
                            "IconLoadingOutline16"
                        },
                        cx + 20.,
                        y + 3.,
                        14.,
                        t.muted,
                        t,
                    );
                    self.label(
                        &mut background,
                        item["content"].as_str().unwrap_or(""),
                        [cx + 42., y, cw - 64., 26.],
                        13.,
                        t.secondary,
                        false,
                        t,
                    );
                }
            }
        }
        background.rect(
            [cx - 3., cy + 2., cw + 6., ch + 4.],
            24.,
            zsui::Color::rgba(0, 0, 0, 5),
        );
        background.rect([cx, cy, cw, ch], 22., t.border);
        background.rect([cx + 1., cy + 1., cw - 2., ch - 2.], 21., t.base);
        self.label(
            &mut background,
            if s.draft.is_empty() {
                "给智能体发消息"
            } else {
                &s.draft
            },
            [cx + 16., cy + 14., cw - 32., 36.],
            16.,
            if s.draft.is_empty() { t.muted } else { t.ink },
            false,
            t,
        );
        for (id, x) in [
            ("IconPlusOutline16", cx + 16.),
            ("IconLinkOutline16", cx + 60.),
            ("IconListPenOutline16", cx + 104.),
        ] {
            self.icon(&mut background, id, x, cy + 65., 16., t.secondary, t);
            self.hit([x - 6., cy + 58., 30., 30.], Action::Open(s.base.clone()));
        }
        let access = match session
            .and_then(|session| session.values.pointer("/permissions/currentValue"))
            .and_then(Value::as_str)
        {
            Some("danger-full-access") => "完全访问",
            Some("read-only") => "只读访问",
            _ => "工作区内修改",
        };
        self.label(
            &mut background,
            access,
            [cx + 124., cy + 62., 110., 24.],
            13.,
            t.secondary,
            false,
            t,
        );
        self.icon(
            &mut background,
            "IconChevronDownOutline14",
            cx + 218.,
            cy + 67.,
            12.,
            t.muted,
            t,
        );
        let model = session
            .map(|r| model::str_at(&r.values, "/modelSelection/model"))
            .filter(|x| !x.is_empty())
            .unwrap_or("选择模型".into());
        if cw > 480. {
            self.label(
                &mut background,
                &model,
                [cx + cw - 275., cy + 62., 182., 25.],
                12.,
                t.secondary,
                false,
                t,
            );
        }
        self.hit([cx + cw - 285., cy + 58., 210., 32.], Action::Model);
        background.rect(
            [cx + cw - 44., cy + 56., 34., 34.],
            13.,
            if session.is_some_and(|r| r.running) {
                t.accent
            } else {
                crate::theme::rgb(0xb4c8f9)
            },
        );
        self.icon(
            &mut background,
            if session.is_some_and(|r| r.running) {
                "IconStopFill16"
            } else {
                "IconSendOutline16"
            },
            cx + cw - 35.,
            cy + 65.,
            16.,
            crate::theme::rgb(0xffffff),
            t,
        );
        self.hit(
            [cx + cw - 48., cy + 54., 40., 40.],
            Action::Open(s.base.clone()),
        );
        if let Some(session) = session {
            let v = &session.values;
            let mut metrics = String::new();
            if let (Some(turns), Some(steps)) = (
                v.pointer("/sessionStats/turns"),
                v.pointer("/sessionStats/steps"),
            ) {
                metrics = format!("{turns} 轮 · {steps} 步");
            }
            if let Some(ms) = v.pointer("/sessionStats/llmMs").and_then(Value::as_u64) {
                metrics.push_str(&format!("  |  LLM {:.1}s", ms as f64 / 1000.));
            }
            if let Some(tok) = v.pointer("/tokenUsage/outputTokens") {
                metrics.push_str(&format!("  |  输出 {tok} tok"));
            }
            self.label(
                &mut background,
                &metrics,
                [cx + 16., cy + ch + 7., cw - 32., 26.],
                11.,
                t.muted,
                false,
                t,
            );
        }
        if !s.status.is_empty() {
            self.label(
                &mut background,
                &s.status,
                [cx, cy - 28., cw, 24.],
                12.,
                t.secondary,
                false,
                t,
            );
        }
        let frame = self.p.frame(background).expect("frame");
        let mut out = vec![(
            boxrect([0., 0., g.w, g.h]),
            zsui::canvas(Self::scene(frame, 0., g.w, g.h))
                .id(WidgetId::new(10))
                .on_canvas_pointer(Msg::Pointer),
        )];
        if g.side > 60. {
            out.push(self.sidebar(s, g, t, scale));
        }
        out.push(self.timeline(s, g, t, scale));
        if let Some(query) = &s.search {
            out.push((
                boxrect([16., 128., g.side - 32., 28.]),
                zsui::textbox(query)
                    .id(WidgetId::new(12))
                    .placeholder("搜索会话")
                    .on_change(Msg::Search),
            ));
        }
        out
    }
    fn sidebar(
        &mut self,
        s: &State,
        g: Geometry,
        t: Theme,
        scale: f32,
    ) -> (ZsCanvasRect, ViewNode<Msg>) {
        let top = if s.search.is_some() { 166. } else { 160. };
        let height = g.h - top - 94.;
        let offset = s.side_scroll;
        let hits_start = self.hits.len();
        let mut r =
            RasterSurface::new(g.side - 1., height, scale, t.sidebar).expect("sidebar buffer");
        let mut y = 0.;
        let mut group_names = Vec::<String>::new();
        for sess in &s.data.sessions {
            if !group_names.contains(&sess.group) {
                group_names.push(sess.group.clone());
            }
        }
        for group in group_names {
            let members: Vec<_> = s
                .data
                .sessions
                .iter()
                .filter(|sess| {
                    sess.group == group
                        && s.search
                            .as_ref()
                            .is_none_or(|q| sess.title.to_lowercase().contains(&q.to_lowercase()))
                })
                .collect();
            if members.is_empty() {
                continue;
            }
            if y - offset >= -32. && y - offset < height {
                self.icon(
                    &mut r,
                    "IconFolderOpen16",
                    18.,
                    y - offset + 8.,
                    16.,
                    t.secondary,
                    t,
                );
                self.label(
                    &mut r,
                    &group,
                    [40., y - offset + 4., g.side - 54., 28.],
                    13.,
                    t.ink,
                    false,
                    t,
                );
                self.hit(
                    [8., top + y - offset, g.side - 16., 32.],
                    Action::Group(group.clone()),
                );
            }
            y += 36.;
            if !s.groups.contains(&group) {
                for sess in members {
                    let sy = y - offset;
                    if sy >= -32. && sy < height {
                        if sess.id == s.data.selected {
                            r.rect([12., sy, g.side - 24., 32.], 7., t.selected);
                        }
                        self.label(
                            &mut r,
                            &sess.title,
                            [40., sy + 5., g.side - 106., 25.],
                            13.,
                            t.ink,
                            false,
                            t,
                        );
                        let elapsed = relative_time(sess.updated);
                        self.label(
                            &mut r,
                            &elapsed,
                            [g.side - 58., sy + 7., 46., 22.],
                            11.,
                            t.muted,
                            false,
                            t,
                        );
                        self.hit(
                            [12., top + sy, g.side - 24., 32.],
                            Action::Session(sess.id.clone()),
                        );
                    }
                    y += 35.;
                }
            }
            y += 12.;
        }
        self.side_total = y.max(height);
        let clamped = offset.clamp(0., (self.side_total - height).max(0.));
        if (clamped - offset).abs() > 0.1 {
            self.hits.truncate(hits_start);
            let mut next = s.clone();
            next.side_scroll = clamped;
            return self.sidebar(&next, g, t, scale);
        }
        let f = self.p.frame(r).expect("sidebar frame");
        let local = boxrect([0., top, g.side - 1., height]);
        let scene = Self::scene(f, offset, g.side - 1., height);
        let origin = top;
        let child = zsui::canvas(scene)
            .id(WidgetId::new(21))
            .height(Dp(self.side_total))
            .on_canvas_pointer_with(move |mut e| {
                e.position.y.0 += origin - offset;
                Msg::Pointer(e)
            });
        (
            local,
            zsui::scroll(child)
                .id(WidgetId::new(20))
                .content_height(Dp(self.side_total))
                .scroll_y(Dp(offset))
                .on_scroll(|v| Msg::SidebarScroll(v.0)),
        )
    }
    fn timeline(
        &mut self,
        s: &State,
        g: Geometry,
        t: Theme,
        scale: f32,
    ) -> (ZsCanvasRect, ViewNode<Msg>) {
        let [left, top, width, height] = g.timeline;
        let mut r = RasterSurface::new(width, height, scale, t.base).expect("timeline buffer");
        let offset = s.scroll;
        let hits_start = self.hits.len();
        let mut y = 22.;
        let x = g.x - left;
        if let Some(error) = &s.data.error {
            self.label(
                &mut r,
                error,
                [x, 24., g.content, 70.],
                14.,
                t.danger,
                false,
                t,
            );
        }
        if s.data.messages.is_empty() {
            self.icon(
                &mut r,
                "FishLogo",
                width / 2. - 17.,
                height / 2. - 60.,
                28.,
                t.ink,
                t,
            );
            self.label(
                &mut r,
                if s.loading {
                    "正在载入会话…"
                } else {
                    "有什么可以帮你？"
                },
                [width / 2. - 120., height / 2. - 12., 300., 48.],
                24.,
                t.ink,
                true,
                t,
            );
        }
        if s.data.more {
            self.label(
                &mut r,
                "加载更早消息",
                [x, y - offset, g.content, 28.],
                13.,
                t.accent,
                false,
                t,
            );
            self.hit([g.x, top + y - offset, g.content, 28.], Action::More);
            y += 38.;
        }
        if s.tab == 4 {
            let val = s
                .data
                .sessions
                .iter()
                .find(|r| r.id == s.data.selected)
                .map(|r| r.values.clone())
                .unwrap_or(Value::Null);
            let blocks = markdown::parse(&format!(
                "## 上下文\n```json\n{}\n```",
                serde_json::to_string_pretty(&val).unwrap_or_default()
            ));
            let l = markdown::layout(
                &blocks,
                g.content,
                &mut self.p,
                &mut self.assets,
                t,
                scale,
                "",
            );
            l.paint(&mut self.p, &mut r, x, y - offset, [0., 0., width, height]);
            y += l.height;
        } else if s.tab == 3 {
            self.label(
                &mut r,
                "代码图谱",
                [x, 32., g.content, 30.],
                22.,
                t.ink,
                true,
                t,
            );
            self.label(
                &mut r,
                "在网页版打开工作区代码图谱",
                [x, 74., g.content, 26.],
                14.,
                t.accent,
                false,
                t,
            );
            self.hit(
                [g.x, top + 70., g.content, 34.],
                Action::Open(s.base.clone()),
            );
            y += 110.;
        } else {
            let cwd = s
                .data
                .sessions
                .iter()
                .find(|r| r.id == s.data.selected)
                .map(|r| r.cwd.as_str())
                .unwrap_or("");
            let mut used = HashSet::new();
            for msg in &s.data.messages {
                if s.tab == 1 && !matches!(msg.role.as_str(), "tool" | "reasoning" | "notice") {
                    continue;
                }
                if s.tab == 2
                    && !msg
                        .blocks
                        .iter()
                        .any(|b| matches!(b, markdown::Block::Image { .. }))
                {
                    continue;
                }
                let collapsed = msg.collapsed && !s.expanded.contains(&msg.id);
                let user = msg.role == "user";
                let mw = if user { g.content.min(560.) } else { g.content };
                let mx = if user { x + g.content - mw } else { x };
                let mut text = msg.text.clone();
                if msg.role == "tool" {
                    text = format!("```json\n{}\n```", msg.text);
                }
                let mut hash = std::hash::DefaultHasher::new();
                msg.text.hash(&mut hash);
                let key = format!(
                    "{}:{}:{}:{scale}:{}:{}:{}",
                    msg.id,
                    mw,
                    s.dark,
                    collapsed,
                    s.tab,
                    hash.finish()
                );
                used.insert(key.clone());
                if !self.layouts.contains_key(&key) {
                    let blocks = if msg.role == "tool" {
                        markdown::parse(&text)
                    } else {
                        msg.blocks.clone()
                    };
                    let l = if collapsed {
                        Layout {
                            height: 38.,
                            ..Layout::default()
                        }
                    } else {
                        markdown::layout(
                            &blocks,
                            if user { mw - 28. } else { mw },
                            &mut self.p,
                            &mut self.assets,
                            t,
                            scale,
                            cwd,
                        )
                    };
                    while self
                        .layouts
                        .values()
                        .map(Layout::estimated_bytes)
                        .sum::<usize>()
                        + l.estimated_bytes()
                        > 8 * 1024 * 1024
                        && !self.layouts.is_empty()
                    {
                        let old = self.layouts.keys().next().unwrap().clone();
                        self.layouts.remove(&old);
                    }
                    self.layouts.insert(key.clone(), l);
                }
                let l = &self.layouts[&key];
                let h = if collapsed {
                    34.
                } else {
                    l.height
                        + if user {
                            20.
                        } else if msg.collapsed {
                            34.
                        } else {
                            0.
                        }
                };
                let visible = y + h > offset && y < offset + height;
                if visible {
                    if user {
                        let a = (y - offset).max(0.);
                        let end = (y - offset + h).min(height);
                        r.rect([mx, a, mw, end - a], 16., t.bubble);
                    }
                    if msg.collapsed {
                        let title = if msg.role == "reasoning" {
                            "思考"
                        } else if msg.error {
                            "工具调用 · 失败"
                        } else {
                            "工具调用"
                        };
                        let sy = y - offset;
                        let col = if msg.error { t.danger } else { t.secondary };
                        r.rect([mx, sy.max(0.), mw, 32.], 8., t.card);
                        self.icon(
                            &mut r,
                            if msg.role == "reasoning" {
                                "IconThinkOutline14"
                            } else {
                                "IconQueueOutline14"
                            },
                            mx + 10.,
                            sy + 8.,
                            14.,
                            col,
                            t,
                        );
                        self.label(
                            &mut r,
                            title,
                            [mx + 32., sy + 4., 120., 25.],
                            13.,
                            col,
                            false,
                            t,
                        );
                        let preview = msg.text.lines().next().unwrap_or("");
                        self.label(
                            &mut r,
                            preview,
                            [mx + 136., sy + 4., mw - 170., 25.],
                            12.,
                            t.muted,
                            false,
                            t,
                        );
                        self.hit(
                            [left + mx, top + sy, mw, 32.],
                            Action::Expand(msg.id.clone()),
                        );
                    }
                    let l = &self.layouts[&key];
                    if !collapsed {
                        let dy = y - offset
                            + if user {
                                10.
                            } else if msg.collapsed {
                                36.
                            } else {
                                0.
                            };
                        l.paint(
                            &mut self.p,
                            &mut r,
                            mx + if user { 14. } else { 0. },
                            dy,
                            [0., 0., width, height],
                        );
                        for (b, text) in &l.copies {
                            let b = [left + mx + b[0], top + dy + b[1], b[2], b[3]];
                            if b[1] >= top && b[1] + b[3] < top + height {
                                self.hits.push((b, Action::Copy(text.clone())));
                            }
                        }
                        for d in &l.draws {
                            if let Draw::Text {
                                b, link: Some(url), ..
                            } = d
                            {
                                let b = [left + mx + b[0], top + dy + b[1], b[2], b[3]];
                                if b[1] >= top && b[1] + b[3] < top + height {
                                    self.hits.push((b, Action::Open(url.clone())));
                                }
                            }
                        }
                    }
                    let copy_y = y - offset + h + 2.;
                    if copy_y >= 0. && copy_y + 24. < height {
                        self.icon(&mut r, "IconCopyOutline16", mx, copy_y, 14., t.muted, t);
                        self.hit(
                            [left + mx - 5., top + copy_y - 3., 24., 24.],
                            Action::Copy(msg.text.clone()),
                        );
                    }
                }
                y += h + if collapsed { 12. } else { 34. };
            }
            self.layouts.retain(|k, _| used.contains(k));
        }
        self.total = y.max(height);
        let clamped = offset.clamp(0., (self.total - height).max(0.));
        if (clamped - offset).abs() > 0.1 {
            self.hits.truncate(hits_start);
            let mut next = s.clone();
            next.scroll = clamped;
            return self.timeline(&next, g, t, scale);
        }
        let f = self.p.frame(r).expect("timeline frame");
        let scene = Self::scene(f, offset, width, height);
        let child = zsui::canvas(scene)
            .id(WidgetId::new(31))
            .height(Dp(self.total))
            .on_canvas_pointer_with(move |mut e| {
                e.position.x.0 += left;
                e.position.y.0 += top - offset;
                Msg::Pointer(e)
            });
        (
            boxrect(g.timeline),
            zsui::scroll(child)
                .id(WidgetId::new(30))
                .content_height(Dp(self.total))
                .scroll_y(Dp(offset))
                .on_scroll(|v| Msg::Scroll(v.0)),
        )
    }
}
fn relative_time(time: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let m = ((now - time).max(0) / 60000) as u64;
    if m < 1 {
        "刚刚".into()
    } else if m < 60 {
        format!("{m}分钟")
    } else if m < 1440 {
        format!("{}小时", m / 60)
    } else {
        format!("{}天", m / 1440)
    }
}
fn refresh(app: &App, selected: Option<String>, before: Option<i64>) {
    let (base, generation, offline) = {
        let mut s = app.state.lock().unwrap();
        s.loading = true;
        s.generation += 1;
        (s.base.clone(), s.generation, s.offline)
    };
    if offline {
        app.state.lock().unwrap().loading = false;
        return;
    }
    let state = app.state.clone();
    let wake = app.wake.clone();
    std::thread::spawn(move || {
        let result = model::load(&base, selected.as_deref(), before);
        let mut s = state.lock().unwrap();
        if s.generation != generation {
            return;
        }
        s.loading = false;
        match result {
            Ok(mut data) => {
                if before.is_some() {
                    data.messages.extend(s.data.messages.clone());
                    data.messages.truncate(300);
                    s.scroll = 0.;
                } else {
                    s.scroll = 0.;
                }
                s.data = data;
                s.status.clear();
            }
            Err(e) => s.data.error = Some(e),
        }
        drop(s);
        wake.request_rebuild();
    });
}
fn update(app: &mut App, msg: Msg) {
    if app.capture_only {
        return;
    }
    match msg {
        Msg::Scroll(v) => app.state.lock().unwrap().scroll = v,
        Msg::SidebarScroll(v) => app.state.lock().unwrap().side_scroll = v,
        Msg::Search(v) => {
            let mut s = app.state.lock().unwrap();
            s.search = Some(v);
            s.side_scroll = 0.;
        }
        Msg::Pointer(e) if e.phase == ZsCanvasPointerPhase::Released && e.inside => {
            let a = app
                .renderer
                .lock()
                .unwrap()
                .hits
                .iter()
                .rev()
                .find(|(b, _)| contains(*b, e.position.x.0, e.position.y.0))
                .map(|(_, a)| a.clone());
            if let Some(a) = a {
                match a {
                    Action::Collapse => {
                        let mut s = app.state.lock().unwrap();
                        s.collapsed = !s.collapsed;
                        s.scroll = 0.;
                    }
                    Action::Group(g) => {
                        let mut s = app.state.lock().unwrap();
                        if !s.groups.remove(&g) {
                            s.groups.insert(g);
                        }
                        s.side_scroll = 0.;
                    }
                    Action::Session(id) => refresh(app, Some(id), None),
                    Action::Refresh => {
                        let id = app.state.lock().unwrap().data.selected.clone();
                        refresh(app, Some(id), None);
                    }
                    Action::More => {
                        let s = app.state.lock().unwrap();
                        let id = s.data.selected.clone();
                        let before = s.data.first;
                        drop(s);
                        refresh(app, Some(id), before);
                    }
                    Action::Theme => {
                        let mut s = app.state.lock().unwrap();
                        s.dark = !s.dark;
                    }
                    Action::Tasks => {
                        let mut s = app.state.lock().unwrap();
                        s.tasks_open = !s.tasks_open;
                    }
                    Action::Tab(i) => {
                        let mut s = app.state.lock().unwrap();
                        s.tab = i;
                        s.scroll = 0.;
                    }
                    Action::Search => {
                        let mut s = app.state.lock().unwrap();
                        s.search = if s.search.is_some() {
                            None
                        } else {
                            Some(String::new())
                        };
                    }
                    Action::Expand(id) => {
                        let mut s = app.state.lock().unwrap();
                        if !s.expanded.remove(&id) {
                            s.expanded.insert(id);
                        }
                    }
                    Action::Copy(text) => {
                        let result = arboard::Clipboard::new().and_then(|mut c| c.set_text(text));
                        app.state.lock().unwrap().status = if result.is_ok() {
                            "已复制".into()
                        } else {
                            "无法访问剪贴板".into()
                        };
                    }
                    Action::Open(url) => open_url(app, &url),
                    Action::New | Action::Model => {
                        let url = app.state.lock().unwrap().base.clone();
                        open_url(app, &url);
                    }
                }
            }
        }
        _ => {}
    }
}
fn open_url(app: &App, url: &str) {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        app.state.lock().unwrap().status = "仅支持打开 HTTP(S) 链接".into();
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("rundll32.exe")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .creation_flags(0x08000000)
            .spawn();
    }
}
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let get = |key: &str| {
        args.iter()
            .position(|x| x == key)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let base = get("--url").unwrap_or("http://127.0.0.1:58080".into());
    let fixture = get("--fixture");
    let data = if let Some(f) = &fixture {
        model::fixture(std::path::Path::new(f))?
    } else {
        Data::default()
    };
    let state = State {
        data,
        base,
        dark: args.iter().any(|s| s == "--dark"),
        collapsed: get("--width")
            .and_then(|v| v.parse::<u32>().ok())
            .is_some_and(|w| w < 850),
        groups: HashSet::new(),
        expanded: HashSet::new(),
        tab: 0,
        scroll: get("--scroll").and_then(|v| v.parse().ok()).unwrap_or(0.),
        side_scroll: 0.,
        search: None,
        draft: String::new(),
        loading: false,
        status: String::new(),
        generation: 0,
        offline: fixture.is_some(),
        tasks_open: false,
    };
    let app = App {
        state: Arc::new(Mutex::new(state)),
        renderer: Arc::new(Mutex::new(Renderer::default())),
        wake: UiInvalidationHandle::new(),
        host: Arc::new(Mutex::new(None)),
        capture_only: get("--smoke").is_some() && !args.iter().any(|a| a == "--exercise"),
    };
    if fixture.is_none() {
        let state = app.state.clone();
        let host = app.host.clone();
        let wake = app.wake.clone();
        state.lock().unwrap().loading = true;
        std::thread::spawn(move || {
            let base = state.lock().unwrap().base.clone();
            let result = crate::host::connect(&base).and_then(|(url, owned)| {
                *host.lock().unwrap() = owned;
                state.lock().unwrap().base = url.clone();
                model::load(&url, None, None)
            });
            let mut s = state.lock().unwrap();
            s.loading = false;
            match result {
                Ok(data) => s.data = data,
                Err(e) => s.data.error = Some(e),
            }
            drop(s);
            wake.request_rebuild();
        });
    }
    let width = get("--width").and_then(|s| s.parse().ok()).unwrap_or(1280);
    let height = get("--height").and_then(|s| s.parse().ok()).unwrap_or(800);
    let renderer = app.renderer.clone();
    let state = app.state.clone();
    let builder = zsui::native_window("DeepSeek Harness Desktop")
        .size(width, height)
        .min_size(720, 540)
        .invalidation_handle(app.wake.clone())
        .stateful_view(
            app,
            |app| {
                let state = app.state.clone();
                let renderer = app.renderer.clone();
                let mode = if state.lock().unwrap().dark {
                    zsui::ZsuiThemeMode::Dark
                } else {
                    zsui::ZsuiThemeMode::Light
                };
                zsui::surface(move |b, d| {
                    let s = state.lock().unwrap().clone();
                    renderer.lock().unwrap().build(&s, b, d)
                })
                .theme_mode(mode)
            },
            |app, msg, _| update(app, msg),
        );
    if let Some(out) = get("--smoke") {
        let path = PathBuf::from(out);
        std::fs::create_dir_all(&path)?;
        let mut options = zsui::NativeWindowSmokeRunOptions::new(1800)
            .screenshot_file(path.join("window.png").to_string_lossy())
            .require_screenshot(true);
        if args.iter().any(|a| a == "--exercise") {
            options = options
                .native_view_click(zsui::Point {
                    x: 28,
                    y: height as i32 - 64,
                })
                .native_view_scroll(zsui::Point { x: 850, y: 300 }, 260)
                .native_view_click(zsui::Point { x: 190, y: 140 })
                .native_view_click_widget(WidgetId::new(12))
                .native_view_text_input("中文");
        }
        if args.iter().any(|a| a == "--resize-test") {
            options = options
                .native_window_resize(zsui::Size {
                    width: 960,
                    height: 640,
                })
                .require_native_window_resize(true);
        }
        let report = builder.run_smoke(options)?;
        let r = renderer.lock().unwrap();
        let snapshot = state.lock().unwrap();
        let evidence = json!({"native":report,"assetsCacheBytes":r.assets.bytes,"textLayoutBytes":r.p.text.layout_cache_bytes(),"glyphCacheBytes":r.p.text.glyph_cache_bytes(),"cachedMessages":r.layouts.len(),"messageCount":snapshot.data.messages.len(),"sessionCount":snapshot.data.sessions.len(),"loadError":snapshot.data.error,"loading":snapshot.loading,"width":width,"height":height});
        std::fs::write(
            path.join("report.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
    } else {
        builder.run()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> State {
        State {
            data: Data::default(),
            base: "http://127.0.0.1:58080".into(),
            dark: false,
            collapsed: false,
            groups: HashSet::new(),
            expanded: HashSet::new(),
            tab: 0,
            scroll: 0.,
            side_scroll: 0.,
            search: None,
            draft: String::new(),
            loading: false,
            status: String::new(),
            generation: 0,
            offline: true,
            tasks_open: false,
        }
    }
    #[test]
    fn responsive_layout_preserves_composer_and_hit_geometry() {
        let mut renderer = Renderer::default();
        let s = state();
        for (w, h, scale) in [(1280, 800, 1.), (960, 640, 1.25), (800, 600, 2.)] {
            let children = renderer.build(
                &s,
                Rect {
                    x: 0,
                    y: 0,
                    width: (w as f32 * scale) as i32,
                    height: (h as f32 * scale) as i32,
                },
                Dpi::new(96. * scale),
            );
            assert_eq!(children.len(), 3);
            assert!(
                renderer
                    .hits
                    .iter()
                    .any(|(_, a)| matches!(a, Action::Collapse))
            );
            let g = Geometry::new(w as f32, h as f32, false);
            assert!(g.composer[0] >= g.side);
            assert!(g.composer[0] + g.composer[2] <= w as f32);
            assert!(g.composer[1] + g.composer[3] < h as f32);
            assert!(renderer.assets.bytes <= 8 * 1024 * 1024);
        }
    }
    #[test]
    fn stale_scroll_is_clamped_without_blank_leading_space() {
        let mut renderer = Renderer::default();
        let mut s = state();
        s.scroll = 100000.;
        s.side_scroll = 100000.;
        let children = renderer.build(
            &s,
            Rect {
                x: 0,
                y: 0,
                width: 1280,
                height: 800,
            },
            Dpi::standard(),
        );
        if let zsui::ViewNodeKind::Scroll { offset_y, .. } = children[2].1.kind {
            assert_eq!(offset_y.0, 0.);
        } else {
            panic!("timeline must remain a native scroll region")
        }
    }
}
