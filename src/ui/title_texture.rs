use anyhow::ensure;
use pango::FontDescription;
use pangocairo::cairo::{self, ImageSurface};
use smithay::backend::allocator::Fourcc;
use smithay::utils::Transform;

use crate::backend::tty_renderer::TtyOffscreen;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::texture::TextureBuffer;
use crate::utils::to_physical_precise_round;

/// Texture with text rendered via pango/cairo, usable with any renderer.
pub type TitleTexture = TextureBuffer<TtyOffscreen>;

/// Generate a texture with the given title.
///
/// `font` is a pango font description string, e.g. `"sans 11px"`.  `color` is the text color in
/// RGB.  `max_width_px` ellipsizes the text to fit within the given width in physical pixels.
#[allow(clippy::too_many_arguments)]
pub fn generate_title_texture<R: NiriRenderer>(
    renderer: &mut R,
    font: &str,
    color: (f64, f64, f64),
    max_width_px: Option<i32>,
    title: &str,
    scale: f64,
) -> anyhow::Result<TitleTexture> {
    let _span = tracy_client::span!("ui::generate_title_texture");

    let mut font = FontDescription::from_string(font);
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    // On Window CSD, line breaks are either stripped or replaced with the linebreak symbol anyway.
    // No use rendering it as multiple lines.
    layout.set_single_paragraph_mode(true);
    layout.set_font_description(Some(&font));
    layout.set_text(title);

    if let Some(max_width_px) = max_width_px {
        layout.set_ellipsize(pango::EllipsizeMode::End);
        layout.set_width(max_width_px * pango::SCALE);
    }

    let (width, height) = layout.pixel_size();
    ensure!(width > 0 && height > 0);

    // Guard against overly long window titles.
    let width = std::cmp::min(width, 16383);
    let height = std::cmp::min(height, 16383);

    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    cr.set_source_rgb(color.0, color.1, color.2);
    pangocairo::functions::show_layout(&cr, &layout);

    drop(cr);
    let data = surface.take_data().unwrap();
    let buffer = TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width, height),
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?;

    Ok(buffer.map_texture(R::wrap_texture))
}
