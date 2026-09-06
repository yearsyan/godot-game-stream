# Store media

All first-party artwork is LGPL-2.1-or-later, copyright (C) 2026 yearsyan and
contributors. The vector thumbnail illustrates the host/viewer relationship; it
is not a product screenshot. The demonstration image is an actual decoded video
frame, without added UI, labels or retouching.

| File | Size | Intended use |
| --- | --- | --- |
| media/thumbnail.png | 1440 x 810 | Store thumbnail or featured image |
| media/icon.png | 512 x 512 | Square asset icon |
| media/stream-demo.png | 1280 x 720 | Actual streamed example screenshot |

Screenshot caption: "A release export of the bundled Godot example decoded from
an H.264 stream on macOS. The status line shows VideoToolbox encoding and a mouse click delivered
over the remote input protocol."

Rebuild the vector-derived media with Python and CairoSVG:

```sh
python3 -m pip install cairosvg
python3 scripts/render_store_media.py --stream build/store-export-macos/stream/sample.h264
```

Keep this directory outside the addon installation ZIP. The parent .gdignore
prevents Godot from importing the store media into the development project.
