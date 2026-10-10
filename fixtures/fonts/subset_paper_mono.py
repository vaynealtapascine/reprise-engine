"""Reproduce PaperMono-TestSubset.ttf with fontTools 4.66.1.

Input: paper-design/paper-mono at e6eaeceaef02e77e3db997711e07a16378de2bd7,
fonts/ttf/PaperMono-Regular.ttf, SHA-256
130e1a09b64b4f150d34c93a6b7bd52d6c5c3513761d1fbc7be42d68398b8c10.
No Reserved Font Name is declared. Keep attribution records and ASCII only.
Usage: python subset_paper_mono.py source.ttf PaperMono-TestSubset.ttf
"""
import sys
from fontTools import subset


def main(source, target):
    options = subset.Options()
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.notdef_outline = True
    options.glyph_names = False
    options.hinting = False
    options.drop_tables = ["DSIG"]
    options.layout_features = ["ccmp", "locl", "liga", "kern", "mark", "mkmk"]
    font = subset.load_font(source, options)
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=range(0x20, 0x7F))
    subsetter.subset(font)
    font["head"].modified = font["head"].created
    subset.save_font(font, target, options)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
