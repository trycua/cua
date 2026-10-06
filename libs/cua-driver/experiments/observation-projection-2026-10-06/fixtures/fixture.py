"""Synthetic AppKit target with independent state and explicit fault injection."""
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(os.environ['ARC_EVAL_SOURCE']) / 'benchmarks'))
import fixture_form
import AppKit
import Foundation
import objc


class EvalForm(fixture_form.Form):
    def build(self):
        objc.super(EvalForm, self).build()
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
