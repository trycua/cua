"""Extract legacy ARIA snapshot bounds without sending captured HTML to the DOM."""

import math
from html.parser import HTMLParser


class _Bounds(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.screen_offset = (0, 0)
        self.boxes = []
        self.pending = None

    def handle_starttag(self, tag, attrs):
        self.pending = None  # Only leaf elements, matching the legacy viewer.
        attrs = dict(attrs)
        try:
            if tag == "html":
                self.screen_offset = tuple(
                    float(attrs.get("data-window-screen-" + axis, 0)) for axis in ("x", "y")
                )
            if "aria-label" not in attrs or len(self.boxes) >= 1000:
                return
            box = {key: float(attrs["data-bbox-" + key]) for key in ("x", "y", "width", "height")}
            box["x"] += self.screen_offset[0]
            box["y"] += self.screen_offset[1]
            if (
                not all(math.isfinite(value) for value in box.values())
                or box["width"] < 0
                or box["height"] < 0
            ):
                return
            box["label"] = str(attrs["aria-label"])[:512]
            if tag in ("input", "img", "br", "hr", "area", "embed", "source", "wbr"):
                self.boxes.append(box)
            else:
                self.pending = box
        except (KeyError, TypeError, ValueError):
            pass

    def handle_endtag(self, tag):
        if self.pending is not None:
            self.boxes.append(self.pending)
            self.pending = None


def snapshot_boxes(data):
    snapshot = data.get("snapshot")
    if not isinstance(snapshot, dict) or not isinstance(snapshot.get("windows"), list):
        return []
    boxes = []
    for window in snapshot["windows"]:
        if isinstance(window, dict) and isinstance(window.get("html"), str):
            parser = _Bounds()
            parser.feed(window["html"])
            boxes.extend(parser.boxes[: 1000 - len(boxes)])
            if len(boxes) >= 1000:
                break
    return boxes
