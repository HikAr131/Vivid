use eframe::egui::{self, Color32, Id, Pos2, TextureHandle, Vec2};

const NAMES: [&str; 3] = ["douyin", "bilibili", "xiaohongshu"];
const PNGS: [&[u8]; 3] = [
    include_bytes!("../assets/social-icons/douyin.png"),
    include_bytes!("../assets/social-icons/bilibili.png"),
    include_bytes!("../assets/social-icons/xiaohongshu.png"),
];

pub fn paint(ui: &egui::Ui, index: usize, center: Pos2, color: Color32) {
    let key = Id::new(("social-icon-texture", index));
    let texture = ui.ctx().data(|data| data.get_temp::<TextureHandle>(key));
    let texture = texture.unwrap_or_else(|| {
        let image = image::load_from_memory(PNGS[index])
            .expect("embedded social icon must be a valid PNG")
            .to_rgba8();
        let texture = ui.ctx().load_texture(
            NAMES[index],
            egui::ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize],
                &image,
            ),
            egui::TextureOptions::LINEAR,
        );
        ui.ctx()
            .data_mut(|data| data.insert_temp(key, texture.clone()));
        texture
    });
    ui.painter().image(
        texture.id(),
        egui::Rect::from_center_size(center, Vec2::splat(16.0)),
        egui::Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_transparent_white_masks_with_visible_cutouts() {
        for bytes in PNGS {
            let image = image::load_from_memory(bytes).unwrap().to_rgba8();
            assert_eq!(image.dimensions(), (96, 96));
            assert_eq!(image.get_pixel(0, 0)[3], 0);
            let mut holes = 0;
            let mut solid = 0;
            for (x, y, pixel) in image.enumerate_pixels() {
                if pixel[3] > 0 {
                    assert_eq!(&pixel.0[..3], &[255, 255, 255]);
                }
                if (12..84).contains(&x) && (12..84).contains(&y) {
                    holes += usize::from(pixel[3] == 0);
                    solid += usize::from(pixel[3] == 255);
                }
            }
            assert!(holes > 100, "cutouts must remain transparent");
            assert!(solid > 1000, "solid rounded-square background missing");
        }
    }

    #[test]
    fn repeated_icons_keep_distinct_textures_and_inherit_muted_color() {
        let ctx = egui::Context::default();
        let tint = Color32::from_rgb(110, 110, 115);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for index in [0, 1, 2, 0, 1, 2] {
                paint(
                    ui,
                    index,
                    egui::pos2(40.0 + index as f32 * 32.0, 40.0),
                    tint,
                );
            }
        });
        let mut ids = std::collections::HashSet::new();
        let mut meshes = 0;
        for shape in &output.shapes {
            if let egui::Shape::Mesh(mesh) = &shape.shape {
                ids.insert(mesh.texture_id);
                assert!(mesh.vertices.iter().all(|vertex| vertex.color == tint));
                meshes += 1;
            }
        }
        assert_eq!(ids.len(), 3);
        assert_eq!(meshes, 6);
        output.textures_delta.clear();
    }
}
