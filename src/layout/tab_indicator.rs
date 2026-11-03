use std::cell::RefCell;
use std::iter::zip;
use std::mem;

use niri_config::{CornerRadius, Gradient, GradientRelativeTo, TabIndicatorPosition};
use smithay::backend::renderer::element::Kind;
use smithay::utils::{Logical, Point, Rectangle, Size, Transform};

use super::tile::Tile;
use super::LayoutElement;
use crate::animation::{Animation, Clock};
use crate::backend::tty_renderer::TtyOffscreen;
use crate::niri_render_elements;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::texture::{
    TextureBuffer, TextureRenderElement, UniversalTextureRenderElement,
};
use crate::ui::title_texture::{generate_title_texture, TitleTexture as TabTitleTexture};
use crate::utils::{
    floor_logical_in_physical_max1, round_logical_in_physical, round_logical_in_physical_max1,
};

#[derive(Debug)]
pub struct TabIndicator {
    shader_locs: Vec<Point<f64, Logical>>,
    shaders: Vec<BorderRenderElement>,
    open_anim: Option<Animation>,
    config: niri_config::TabIndicator,
    /// Sizes of the per-tab rects, parallel to `shaders`.
    tab_sizes: Vec<Size<f64, Logical>>,
    /// Per-tab titles, parallel to `shaders`.
    titles: Vec<Option<String>>,
    /// Per-tab active flags for the title color, parallel to `shaders`.
    titles_active: Vec<bool>,
    /// Cached title textures, parallel to `shaders`.
    title_textures: RefCell<Vec<TitleTextureCache>>,
    /// Scale of the last `update_render_elements()` call.
    scale: f64,
}

#[derive(Debug)]
pub struct TabInfo {
    /// Gradient for the tab indicator.
    pub gradient: Gradient,
    /// Tab geometry in the same coordinate system as the area.
    pub geometry: Rectangle<f64, Logical>,
    /// Title of the tab's window.
    pub title: Option<String>,
    /// Whether the tab's window is the active one.
    pub is_active: bool,
}

/// Cached title texture for one tab.
#[derive(Debug, Default)]
struct TitleTextureCache {
    key: (Option<String>, bool, i32, f64),
    /// Cached generated texture, without the `TextureBuffer` wrapper: the buffer is
    /// rebuilt fresh every frame so that the element gets a new Id and is redrawn.
    texture: Option<Option<TtyOffscreen>>,
}

niri_render_elements! {
    TabIndicatorRenderElement => {
        Gradient = BorderRenderElement,
        Texture = UniversalTextureRenderElement,
    }
}

impl TabIndicator {
    pub fn new(config: niri_config::TabIndicator) -> Self {
        Self {
            shader_locs: Vec::new(),
            shaders: Vec::new(),
            open_anim: None,
            config,
            tab_sizes: Vec::new(),
            titles: Vec::new(),
            titles_active: Vec::new(),
            title_textures: RefCell::new(Vec::new()),
            scale: 1.,
        }
    }

    pub fn update_config(&mut self, config: niri_config::TabIndicator) {
        self.config = config;
        self.title_textures.get_mut().clear();
    }

    pub fn update_shaders(&mut self) {
        for elem in &mut self.shaders {
            elem.damage_all();
        }
    }

    /// Called when some window title changed; drops title texture caches.
    pub fn titles_changed(&mut self) {
        self.update_shaders();
        self.title_textures.get_mut().clear();
    }

    pub fn advance_animations(&mut self) {
        if let Some(anim) = &mut self.open_anim {
            if anim.is_done() {
                self.open_anim = None;
            }
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.open_anim.is_some()
    }

    pub fn start_open_animation(&mut self, clock: Clock, config: niri_config::Animation) {
        self.open_anim = Some(Animation::new(clock, 0., 1., 0., config));
    }

    fn tab_rects(
        &self,
        area: Rectangle<f64, Logical>,
        count: usize,
        scale: f64,
    ) -> impl Iterator<Item = Rectangle<f64, Logical>> {
        let round = |logical: f64| round_logical_in_physical(scale, logical);
        let round_max1 = |logical: f64| round_logical_in_physical_max1(scale, logical);

        let progress = self.open_anim.as_ref().map_or(1., |a| a.value().max(0.));

        let width = round_max1(self.config.width);
        let gap = self.config.gap;
        let gap = round_max1(gap.abs()).copysign(gap);
        let gaps_between = round_max1(self.config.gaps_between_tabs);

        let position = self.config.position;
        let side = match position {
            TabIndicatorPosition::Left | TabIndicatorPosition::Right => area.size.h,
            TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => area.size.w,
        };
        let total_prop = self.config.length.total_proportion.unwrap_or(0.5);
        let min_length = round(side * total_prop.clamp(0., 2.));

        // Compute px_per_tab before applying the animation to gaps_between in order to avoid it
        // growing and shrinking over the duration of the animation.
        let pixel = 1. / scale;
        let shortest_length = count as f64 * (pixel + gaps_between) - gaps_between;
        let length = f64::max(min_length, shortest_length);
        let px_per_tab = (length + gaps_between) / count as f64 - gaps_between;

        let px_per_tab = px_per_tab * progress;
        let gaps_between = round(self.config.gaps_between_tabs * progress);

        let length = count as f64 * (px_per_tab + gaps_between) - gaps_between;
        let px_per_tab = floor_logical_in_physical_max1(scale, px_per_tab);
        let floored_length = count as f64 * (px_per_tab + gaps_between) - gaps_between;
        let mut ones_left = ((length - floored_length) / pixel).round() as usize;

        let mut shader_loc = Point::from((-gap - width, round((side - length) / 2.)));
        match position {
            TabIndicatorPosition::Left => (),
            TabIndicatorPosition::Right => shader_loc.x = area.size.w + gap,
            TabIndicatorPosition::Top => mem::swap(&mut shader_loc.x, &mut shader_loc.y),
            TabIndicatorPosition::Bottom => {
                shader_loc.x = shader_loc.y;
                shader_loc.y = area.size.h + gap;
            }
        }
        shader_loc += area.loc;

        (0..count).map(move |_| {
            let mut px_per_tab = px_per_tab;
            if ones_left > 0 {
                ones_left -= 1;
                px_per_tab += pixel;
            }

            let loc = shader_loc;

            match position {
                TabIndicatorPosition::Left | TabIndicatorPosition::Right => {
                    shader_loc.y += px_per_tab + gaps_between
                }
                TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => {
                    shader_loc.x += px_per_tab + gaps_between
                }
            }

            let size = match position {
                TabIndicatorPosition::Left | TabIndicatorPosition::Right => {
                    Size::from((width, px_per_tab))
                }
                TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => {
                    Size::from((px_per_tab, width))
                }
            };

            Rectangle::new(loc, size)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_render_elements(
        &mut self,
        enabled: bool,
        // Geometry of the tabs area.
        area: Rectangle<f64, Logical>,
        // View rect relative to the tabs area.
        area_view_rect: Rectangle<f64, Logical>,
        // Tab count, should match the tabs iterator length.
        tab_count: usize,
        tabs: impl Iterator<Item = TabInfo>,
        is_active: bool,
        scale: f64,
    ) {
        self.scale = scale;

        if !enabled || self.config.off {
            self.shader_locs.clear();
            self.shaders.clear();
            self.tab_sizes.clear();
            self.titles.clear();
            self.titles_active.clear();
            self.title_textures.get_mut().clear();
            return;
        }

        let count = tab_count;
        if self.config.hide_when_single_tab && count == 1 {
            self.shader_locs.clear();
            self.shaders.clear();
            self.tab_sizes.clear();
            self.titles.clear();
            self.titles_active.clear();
            self.title_textures.get_mut().clear();
            return;
        }

        self.shaders.resize_with(count, Default::default);
        self.shader_locs.resize_with(count, Default::default);
        self.tab_sizes.resize_with(count, Default::default);
        self.titles.resize_with(count, Default::default);
        self.titles_active.resize_with(count, Default::default);
        self.title_textures.get_mut().resize_with(count, Default::default);

        let position = self.config.position;
        let radius = self.config.corner_radius as f32;
        let shared_rounded_corners = self.config.gaps_between_tabs == 0.;
        let mut tabs_left = tab_count;

        let rects = self.tab_rects(area, count, scale);
        for (idx, (_tabs_left, (shader, (loc, (tab, rect))))) in zip(
            (0..tab_count).rev(),
            zip(&mut self.shaders,
            zip(&mut self.shader_locs,
            zip(tabs, rects))),
        ).enumerate() {
            *loc = rect.loc;
            self.tab_sizes[idx] = rect.size;
            self.titles[idx] = tab.title;
            self.titles_active[idx] = tab.is_active;

            let mut gradient_area = match tab.gradient.relative_to {
                GradientRelativeTo::Window => tab.geometry,
                GradientRelativeTo::WorkspaceView => area_view_rect,
            };
            gradient_area.loc -= *loc;

            let mut color_from = tab.gradient.from;
            let mut color_to = tab.gradient.to;
            if !is_active {
                color_from *= 0.5;
                color_to *= 0.5;
            }

            let radius = if shared_rounded_corners && tab_count > 1 {
                if tabs_left == tab_count {
                    // First tab.
                    match position {
                        TabIndicatorPosition::Left | TabIndicatorPosition::Right => CornerRadius {
                            top_left: radius,
                            top_right: radius,
                            bottom_right: 0.,
                            bottom_left: 0.,
                        },
                        TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => CornerRadius {
                            top_left: radius,
                            top_right: 0.,
                            bottom_right: 0.,
                            bottom_left: radius,
                        },
                    }
                } else if tabs_left == 1 {
                    // Last tab.
                    match position {
                        TabIndicatorPosition::Left | TabIndicatorPosition::Right => CornerRadius {
                            top_left: 0.,
                            top_right: 0.,
                            bottom_right: radius,
                            bottom_left: radius,
                        },
                        TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => CornerRadius {
                            top_left: 0.,
                            top_right: radius,
                            bottom_right: radius,
                            bottom_left: 0.,
                        },
                    }
                } else {
                    // Tab in the middle.
                    CornerRadius::default()
                }
            } else {
                // Separate tabs, or the only tab.
                CornerRadius::from(radius)
            };
            let radius = radius.fit_to(rect.size.w as f32, rect.size.h as f32);
            tabs_left -= 1;

            shader.update(
                rect.size,
                gradient_area,
                tab.gradient.in_,
                color_from,
                color_to,
                ((tab.gradient.angle as f32) - 90.).to_radians(),
                Rectangle::from_size(rect.size),
                0.,
                radius,
                scale as f32,
                1.,
            );
        }
    }

    pub fn hit(
        &self,
        area: Rectangle<f64, Logical>,
        tab_count: usize,
        scale: f64,
        point: Point<f64, Logical>,
    ) -> Option<usize> {
        if self.config.off {
            return None;
        }

        let count = tab_count;
        if self.config.hide_when_single_tab && count == 1 {
            return None;
        }

        self.tab_rects(area, count, scale)
            .enumerate()
            .find_map(|(idx, rect)| rect.contains(point).then_some(idx))
    }

    pub fn render(
        &self,
        renderer: &mut impl NiriRenderer,
        pos: Point<f64, Logical>,
        push: &mut dyn FnMut(TabIndicatorRenderElement),
    ) {
        let has_border_shader = BorderRenderElement::has_shader(renderer);
        if !has_border_shader {
            return;
        }

        // Render text first so it sits on top of the tab background bar.
        self.render_titles(renderer, pos, push);

        for (shader, loc) in zip(&self.shaders, &self.shader_locs) {
            let elem = shader.clone().with_location(pos + *loc);
            push(TabIndicatorRenderElement::from(elem));
        }
    }

    fn render_titles(
        &self,
        renderer: &mut impl NiriRenderer,
        pos: Point<f64, Logical>,
        push: &mut dyn FnMut(TabIndicatorRenderElement),
    ) {
        let Some(title_config) = &self.config.title else {
            return;
        };

        // Drawing titles into vertical indicators would need vertical text which we don't
        // support for now.
        if !matches!(
            self.config.position,
            TabIndicatorPosition::Top | TabIndicatorPosition::Bottom
        ) {
            return;
        }

        let scale = self.scale;
        let padding = title_config.padding;

        let mut caches = self.title_textures.borrow_mut();

        for (idx, (title, active)) in zip(&self.titles, &self.titles_active).enumerate() {
            let Some(title) = title else {
                continue;
            };
            let Some(size) = self.tab_sizes.get(idx) else {
                continue;
            };
            let Some(loc) = self.shader_locs.get(idx) else {
                continue;
            };

            let avail_w = (size.w - 2. * padding).max(0.);
            let max_w = (avail_w * scale).round() as i32;
            if max_w <= 0 {
                continue;
            }

            let color = if *active {
                title_config.active_color
            } else {
                title_config.inactive_color
            };

            let cache = &mut caches[idx];
            let key = (Some(title.clone()), *active, max_w, scale);
            if cache.key != key {
                cache.key = key;
                cache.texture = None;
            }

            let texture = cache
                .texture
                .get_or_insert_with(|| {
                    generate_title_texture(
                        renderer,
                        &title_config.font,
                        (color.r as f64, color.g as f64, color.b as f64),
                        Some(max_w),
                        title,
                        scale,
                    )
                    .ok()
                    .map(|buf| buf.texture().clone())
                })
                .clone();

            let Some(texture) = texture else {
                continue;
            };

            // Wrap the cached texture in a fresh TextureBuffer every frame: a new Id
            // means the damage tracker treats it as a new element and redraws it.
            // The bar underneath damages its region on every frame (its shader
            // element bumps its commit counter on each update), and a texture
            // element with stale damage would never be drawn on top of it again.
            let buffer =
                TextureBuffer::from_texture(renderer, texture, scale, Transform::Normal, Vec::new());

            let loc = pos + *loc;
            let texture_size = buffer.logical_size();
            let y = loc.y + (size.h - texture_size.h) / 2.;
            let loc = Point::from((loc.x + padding, y));

            let elem = UniversalTextureRenderElement(TextureRenderElement::from_texture_buffer(
                buffer,
                loc,
                1.,
                None,
                None,
                Kind::Unspecified,
            ));
            push(TabIndicatorRenderElement::Texture(elem));
        }
    }

    /// Extra size occupied by the tab indicator.
    pub fn extra_size(&self, tab_count: usize, scale: f64) -> Size<f64, Logical> {
        if self.config.off
            || !self.config.place_within_column
            || (self.config.hide_when_single_tab && tab_count == 1)
        {
            return Size::from((0., 0.));
        }

        let round = |logical: f64| round_logical_in_physical(scale, logical);
        let width = round(self.config.width);
        let gap = round(self.config.gap);

        // No, I am *not* falling into the rabbit hole of "what if the tab indicator is wide enough
        // that it peeks from the other side of the window".
        let size = f64::max(0., width + gap);

        match self.config.position {
            TabIndicatorPosition::Left | TabIndicatorPosition::Right => Size::from((size, 0.)),
            TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => Size::from((0., size)),
        }
    }

    /// Offset of the tabbed content due to space occupied by the tab indicator.
    pub fn content_offset(&self, tab_count: usize, scale: f64) -> Point<f64, Logical> {
        match self.config.position {
            TabIndicatorPosition::Left | TabIndicatorPosition::Top => {
                self.extra_size(tab_count, scale).to_point()
            }
            TabIndicatorPosition::Right | TabIndicatorPosition::Bottom => Point::from((0., 0.)),
        }
    }

    pub fn config(&self) -> &niri_config::TabIndicator {
        &self.config
    }
}

impl TabInfo {
    pub fn from_tile<W: LayoutElement>(
        tile: &Tile<W>,
        position: Point<f64, Logical>,
        is_active: bool,
        is_urgent: bool,
        config: &niri_config::TabIndicator,
    ) -> Self {
        let rules = tile.window().rules();
        let rule = rules.tab_indicator;

        let gradient_from_rule = || {
            let (color, gradient) = if is_urgent {
                (rule.urgent_color, rule.urgent_gradient)
            } else if is_active {
                (rule.active_color, rule.active_gradient)
            } else {
                (rule.inactive_color, rule.inactive_gradient)
            };
            let color = color.map(Gradient::from);
            gradient.or(color)
        };

        let gradient_from_config = || {
            let (color, gradient) = if is_urgent {
                (config.urgent_color, config.urgent_gradient)
            } else if is_active {
                (config.active_color, config.active_gradient)
            } else {
                (config.inactive_color, config.inactive_gradient)
            };
            let color = color.map(Gradient::from);
            gradient.or(color)
        };

        let gradient_from_border = || {
            // Come up with tab indicator gradient matching the focus ring or the border, whichever
            // one is enabled.
            let focus_ring_config = tile.focus_ring().config();
            let border_config = tile.border().config();
            let config = if focus_ring_config.off {
                border_config
            } else {
                focus_ring_config
            };

            let (color, gradient) = if is_urgent {
                (config.urgent_color, config.urgent_gradient)
            } else if is_active {
                (config.active_color, config.active_gradient)
            } else {
                (config.inactive_color, config.inactive_gradient)
            };
            gradient.unwrap_or_else(|| Gradient::from(color))
        };

        let gradient = gradient_from_rule()
            .or_else(gradient_from_config)
            .unwrap_or_else(gradient_from_border);

        let geometry = Rectangle::new(position, tile.animated_tile_size());

        let title = tile.window().title();

        TabInfo {
            gradient,
            geometry,
            title,
            is_active,
        }
    }
}
