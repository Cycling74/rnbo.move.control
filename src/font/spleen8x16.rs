pub const SPLEEN_8X16: ::embedded_graphics::mono_font::MonoFont = ::embedded_graphics::mono_font::MonoFont {
    image: ::embedded_graphics::image::ImageRaw::new(
        include_bytes!("spleen_8x16.data"),
        128u32,
    ),
    glyph_mapping: &::embedded_graphics::mono_font::mapping::StrGlyphMapping::new(
        "\0 \u{7f}\0\u{a0}ſƒ\0ǍǔǢǣ\0ǦǭǰǴǵ\0ǼȗȞȟ\0ȦȩȮȯ˘˙\0˛˝\u{306}\u{308}ΓΘΣΦΩαδεπστφЁІЇЎ\0АяёіїўҐґ‖‘’“”•…‹›‼ⁿ₧€\0←↕↨∙√∞∩∪≈≡≤≥⌂⌐⌙⌠⌡\0─■▬▲▼◆◊○●◘◙\0◢◥\0☰☷\0☺☼♀♂♠♣♥♦♪♫\0⠀⣿⬆⬇\0⭠⭥\0\u{e0a0}\u{e0a2}\0\u{e0b0}\u{e0b3}",
        31usize,
    ),
    character_size: ::embedded_graphics::geometry::Size::new(8u32, 16u32),
    character_spacing: 0u32,
    baseline: 11u32,
    underline: ::embedded_graphics::mono_font::DecorationDimensions::new(13u32, 1u32),
    strikethrough: ::embedded_graphics::mono_font::DecorationDimensions::new(8u32, 1u32),
};
