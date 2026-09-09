# site

The sales page. One static file plus its images — no build step, no dependencies.
Drop the folder on Netlify, Cloudflare Pages, Vercel or GitHub Pages and it works.

    site/
      index.html    the whole page: markup, CSS, and the scripts that run the panels
      img/          screenshots from docs/design, the two photographs, the share card
      robots.txt
      sitemap.xml

## What is live rather than a screenshot

Five of the panels are rebuilt in HTML on this page rather than shown as stills, so a
visitor watches them work:

| Panel | How it runs |
|---|---|
| Relative | Its own clock. A four-phase loop: green, a car pits, a crew fuel target arrives, BOX BOX. Pausable, and it stops when scrolled off screen. |
| Fuel | Scrubbed by scroll position — the armed load, the laps of fuel and the spare all follow the scrollbar. |
| Radar bars | Scrubbed. A car comes up the inside, goes slate → amber → red, and lights the inboard rail on overlap. |
| Faster class | Scrubbed. The gap closes from 5.6 s, the card turns, the marker slides onto you. |
| Pit stall | Scrubbed. The bar empties to the marks, then overshoots. |

`prefers-reduced-motion` freezes all five on a representative frame instead.

If a panel's real design changes, the HTML in `index.html` has to change with it — that
is the cost of showing it live. The blocks are commented and self-contained.

## Regenerating the screenshots

The page uses the premium captures in `docs/design/`. To reshoot one, point `APPDATA` at
a scratch directory holding a `race/race-overlay.toml` with only the panel you want
visible at `pos = [60.0, 60.0]` (and the same `watch_pos`, which is what a spectating
capture uses), then:

    race-overlay.exe --demo --demo-page=fuel --demo-state=spectating --screenshot=out.png

That writes the whole framebuffer composited onto black, so crop to the widget's bounding
box afterwards. Copy the result into `site/img/`.

## Before it goes live

| Where | What to change |
|---|---|
| `https://raceoverlay.app/` | The real domain. It appears in the canonical link, four `og:`/`twitter:` tags, both JSON-LD blocks, `robots.txt` and `sitemap.xml`. There is a comment at the top of `index.html` marking it. |
| `href="https://polar.sh/FlynnFc"` (×3) | The real Polar checkout link. |
| `$14` | The price, if it moves: the nav, the hero button, the ticket, the comparison table, two FAQ answers and both JSON-LD blocks. |
| `14 days to change your mind` | The refund window you actually want to offer. |

No analytics are in the page. Add whatever you use just before `</body>`.

Fonts come from Google Fonts (Barlow Condensed 600/700, IBM Plex Mono 400/500/600, Inter
400/500/600) with `preconnect` and `display=swap`. To self-host them into `fonts/` instead,
you need those six weights as woff2 — the TTFs in `assets/fonts/` only carry three of them,
so the rest would be synthesised and the type would visibly suffer.
