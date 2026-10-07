"""Synthetic AppKit target with independent state and explicit fault injection."""
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(os.environ['REFERENCE_EVAL_SOURCE']) / 'benchmarks'))
import fixture_form
import AppKit
import Foundation
import objc


# Construct the synthetic window on the selected display before ordering it front.
# Display identity comes from the owned off-screen controller, never a fallback.
if os.environ.get("CUA_TEST_DISPLAY_ID"):
    import ast, textwrap
    def offscreen_content_rect(self, width, height):
        display = int(os.environ["CUA_TEST_DISPLAY_ID"])
        screens = [screen for screen in AppKit.NSScreen.screens()
                   if int(screen.deviceDescription()["NSScreenNumber"]) == display]
        if len(screens) != 1:
            raise RuntimeError("Owned off-screen display is unavailable")
        frame = screens[0].frame()
        return Foundation.NSMakeRect(frame.origin.x + 40, frame.origin.y + 100, width, height)
    module_source = Path(fixture_form.__file__).read_text()
    form_node = next(node for node in ast.parse(module_source).body if isinstance(node, ast.ClassDef) and node.name == "Form")
    build_node = next(node for node in form_node.body if isinstance(node, ast.FunctionDef) and node.name == "build")
    source = textwrap.dedent(ast.get_source_segment(module_source, build_node))
    needle = "Foundation.NSMakeRect(80, 120, 420, height)"
    if source.count(needle) != 1:
        raise RuntimeError("Fixture construction changed; refusing physical-screen fallback")
    source = source.replace(needle, "self.offscreen_content_rect(420, height)")
    show = "self.window.orderFrontRegardless()"
    if source.count(show) != 1:
        raise RuntimeError("Fixture visibility changed; refusing physical-screen fallback")
    source = source.replace(show, "self.window.setFrameOrigin_(self.offscreen_content_rect(420, height).origin)\n        " + show)
    exec(source, fixture_form.__dict__)
    fixture_form.Form.build = fixture_form.__dict__["build"]
    fixture_form.Form.offscreen_content_rect = offscreen_content_rect


class EvalForm(fixture_form.Form):
    def build(self):
        objc.super(EvalForm, self).build()
        self.window.setTitle_("Reference Bench Form")
        self.submit_button = next(v for v in self.window.contentView().subviews()
                                  if isinstance(v, AppKit.NSButton) and v.title() == 'Submit')
        self.record_label = AppKit.NSTextField.labelWithString_('Record A')
        self.record_label.setFrame_(Foundation.NSMakeRect(20, 5, 200, 20))
        self.window.contentView().addSubview_(self.record_label)
        self.command_path = self.path.with_suffix('.command')
        self.applied = 0
        self.submitted_record = None

    def submit_(self, sender):
        self.submitted_record = str(self.record_label.stringValue())
        objc.super(EvalForm, self).submit_(sender)

    def state(self):
        return {**objc.super(EvalForm, self).state(), 'applied': self.applied,
                'record': str(self.record_label.stringValue()),
                'submitted_record': self.submitted_record,
                'submit_label': str(self.submit_button.title())}

    def tick_(self, timer):
        if self.command_path.exists():
            data = json.loads(self.command_path.read_text())
            self.command_path.unlink()
            action = data['action']
            if action == 'label':
                self.submit_button.setTitle_('Changed Submit')
            elif action == 'record':
                self.record_label.setStringValue_('Record B')
            elif action == 'sheet':
                self.openDialog_(None)
            elif action == 'disable':
                self.submit_button.setEnabled_(False)
            elif action == 'second':
                self.buildSecondWindow()
            self.applied = data['id']
        objc.super(EvalForm, self).tick_(timer)


if __name__ == '__main__':
    app = AppKit.NSApplication.sharedApplication()
    app.setActivationPolicy_(AppKit.NSApplicationActivationPolicyRegular)
    activity = Foundation.NSProcessInfo.processInfo().beginActivityWithOptions_reason_(
        Foundation.NSActivityUserInitiated | Foundation.NSActivityLatencyCritical, 'synthetic driver evaluation')
    form = EvalForm.alloc().initWithPath_rows_(sys.argv[1], 0)
    form.build()
    form.buildMenu()
    form.start = sys.argv[2] if len(sys.argv) > 2 else 'shown'
    if form.start != 'shown':
        Foundation.NSTimer.scheduledTimerWithTimeInterval_target_selector_userInfo_repeats_(
            .3, form, 'applyStart:', None, False)
    app.run()
