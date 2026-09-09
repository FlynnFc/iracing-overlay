# Interface icons

Marks from [Lucide](https://lucide.dev), used under the ISC licence — see
`LICENSE-lucide.txt`. Lucide is a community fork of Feather Icons, which
carries the same terms.


## One local change

Lucide ships every icon with `stroke="currentColor"`, which relies on the CSS
cascade to pick up a colour from its container. The overlay renders these
through `resvg`, which has no cascade: `currentColor` resolves to nothing and
the icon comes out blank.

Each file here therefore has `currentColor` rewritten to `#FFFFFF`. White is
neutral under egui's multiplicative tint, so a single copy of each icon still
renders in whatever colour the widget asks for — see `ui::icons::svg`.

If you add an icon, apply the same substitution or it will render invisibly.

## Adding one

Drop `<name>.svg` in this folder and reference it by that name. Files are
looked up beside the executable first, then in the working directory, the same
way `assets/logos/` is.
