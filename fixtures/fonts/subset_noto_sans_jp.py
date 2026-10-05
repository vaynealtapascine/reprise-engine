"""Reproduce NotoSansJP-VerticalSubset.otf, the vertical-text test face.

Input: NotoSansJP-Regular.otf from notofonts/noto-cjk at commit
165c01b46ea533872e002e0785ff17e44f6d97d8 (Sans/SubsetOTF/JP), SHA-256
dff723ba59d57d136764a04b9b2d03205544f7cd785a711442d6d2d085ac5073.
Requires fontTools 4.66.1 (`pip install fonttools==4.66.1`).

    python subset_noto_sans_jp.py NotoSansJP-Regular.otf NotoSansJP-VerticalSubset.otf

The subset keeps the horizontal and vertical layout features listed below
(including vert, vrt2, vkrn and vpal) and the vhea, vmtx and VORG tables, so
tests exercise real vertical metrics. Noto Sans JP declares no Reserved Font Name, so the subset keeps its
name under the SIL OFL 1.1.
"""

import sys

from fontTools import subset

# ASCII, Latin-1 punctuation used in tests, general punctuation, CJK
# punctuation, hiragana, katakana, the fullwidth forms, and a handful of
# ideographs for the fixtures ("紙葉の家", a vertical column of a House of
# Leaves page, numbers and dates).
RANGES = [
    (0x0020, 0x007E),
    (0x00A0, 0x00A0),
    (0x2010, 0x2026),
    (0x3000, 0x303F),
    (0x3041, 0x3096),
    (0x309B, 0x309E),
    (0x30A1, 0x30FE),
    (0xFF01, 0xFF5E),
]
IDEOGRAPHS = "縦書日本語文章横組中央数字年月日紙葉家迷宮廊下暗闇扉部屋一二三四五六七八九十百千万頁"


def unicodes():
    out = set()
    for lo, hi in RANGES:
        out.update(range(lo, hi + 1))
    out.update(ord(c) for c in IDEOGRAPHS)
    return sorted(out)


def main(source, target):
    options = subset.Options()
    options.layout_features = [
        "ccmp", "locl", "liga", "kern", "mark", "palt", "halt", "hwid", "fwid",
        "vert", "vrt2", "vkrn", "vpal", "vhal",
    ]
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.notdef_outline = True
    options.glyph_names = False
    options.hinting = False
    options.drop_tables = ["DSIG"]
    font = subset.load_font(source, options)
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=unicodes())
    subsetter.subset(font)
    # A fixed timestamp keeps the output byte-identical between runs.
    font["head"].modified = font["head"].created
    subset.save_font(font, target, options)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
