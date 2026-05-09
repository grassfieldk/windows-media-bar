fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let svg_path = std::path::Path::new(&manifest).join("assets/icon.svg");
        let out_dir  = std::env::var("OUT_DIR").unwrap();
        let ico_path = std::path::Path::new(&out_dir).join("icon.ico");

        build_ico(&svg_path, &ico_path);

        let mut res = winres::WindowsResource::new();
        res.set_icon(ico_path.to_str().unwrap());
        res.compile().unwrap();
    }
}

fn build_ico(svg_path: &std::path::Path, ico_path: &std::path::Path) {
    let svg_data = std::fs::read(svg_path).expect("assets/icon.svg not found");
    let opt  = usvg::Options::default();
    let tree = usvg::Tree::from_data(&svg_data, &opt).expect("failed to parse SVG");

    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);

    for &size in &[256u32, 48, 32, 16] {
        let scale = size as f32 / 256.0;
        let mut pixmap = tiny_skia::Pixmap::new(size, size).unwrap();
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let image = ico::IconImage::from_rgba_data(size, size, pixmap.data().to_vec());
        icon_dir.add_entry(ico::IconDirEntry::encode(&image).unwrap());
    }

    let mut file = std::fs::File::create(ico_path).unwrap();
    icon_dir.write(&mut file).unwrap();
}
