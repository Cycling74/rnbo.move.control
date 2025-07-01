pub const CGA_8X16: ::embedded_graphics::mono_font::MonoFont = ::embedded_graphics::mono_font::MonoFont {
    image: ::embedded_graphics::image::ImageRaw::new(
        include_bytes!("cga_8x16.data"),
        128u32,
    ),
    glyph_mapping: &::embedded_graphics::mono_font::mapping::StrGlyphMapping::new(
        "\0 \u{7f}\0\u{a0}ſƒơƷ\0Ǻǿ\0Șțɑɸˆˇˉ\0˘˝;\0΄ΊΌ\0ΎΡ\0Σώϐϴ\0ЀџҐґ־\0את\0װ״ᴛᴦᴨ\0ẀẅẟỲỳ‐\0‒―\0‗•…‧‰′″‵‹›‼\0‾⁀⁄⁔\0⁴⁻ⁿ\0₁₋₣₤₧₪€℅ℓ№™Ω℮⅐⅑\0⅓⅞\0←↕↨∂∅∆∈∏∑−∕∙√∞∟∩∫≈≠≡≤≥⊙⌀⌂⌐⌠⌡─│┌┐└┘├┤┬┴┼\0═╬▀▁▄█▌\0▐▓■□\0▪▬▲►▼◄◊○●◘◙◦\0☺☼♀♂♠♣♥♦♪♫✓ﬁﬂ�",
        748usize,
    ),
    character_size: ::embedded_graphics::geometry::Size::new(8u32, 16u32),
    character_spacing: 0u32,
    baseline: 13u32,
    underline: ::embedded_graphics::mono_font::DecorationDimensions::new(15u32, 1u32),
    strikethrough: ::embedded_graphics::mono_font::DecorationDimensions::new(8u32, 1u32),
};
