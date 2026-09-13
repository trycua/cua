#!/usr/bin/env python3
"""Backend and sibling-client checks without GTK or an optional backend typelib."""

import importlib.util
from pathlib import Path
import sys
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import Mock, patch


class ForegroundSiblingsTest(unittest.TestCase):
    def setUp(self):
        gi = ModuleType("gi")
        gi.require_version = Mock()
        repository = ModuleType("gi.repository")
        self.display = SimpleNamespace(__gtype__="GdkWaylandDisplay")
        repository.Gdk = SimpleNamespace(Display=SimpleNamespace(
            get_default=Mock(return_value=self.display)))
        repository.GObject = SimpleNamespace(type_name=Mock(side_effect=lambda value: value))
        repository.GLib = SimpleNamespace()
        repository.Gtk = SimpleNamespace(main=Mock())
        spec = importlib.util.spec_from_file_location(
            "foreground_siblings", Path(__file__).with_name("foreground_siblings.py"))
        self.fixture = importlib.util.module_from_spec(spec)
        with patch.dict(sys.modules, {"gi": gi, "gi.repository": repository}):
            spec.loader.exec_module(self.fixture)
        self.assertEqual(gi.require_version.call_args_list,
                         [unittest.mock.call("Gtk", "3.0"),
                          unittest.mock.call("Gdk", "3.0")])
        self.actors = []

    def actor(self, name, directory):
        actor = SimpleNamespace(name=name, window=Mock(), record=Mock(), journal=Mock())
        actor.window.get_display.return_value = self.display
        self.actors.append(actor)
        return actor

    def run_fixture(self, actor_factory=None):
        with patch.object(sys, "argv", ["foreground_siblings", "--journal-dir", "."]), \
                patch.object(self.fixture, "Actor", side_effect=actor_factory or self.actor), \
                patch.object(self.fixture.os, "getpid", return_value=1234):
            self.fixture.main()

    def test_wayland_siblings_share_actual_display_and_pid(self):
        self.run_fixture()
        self.assertEqual([actor.name for actor in self.actors], ["Target", "Sibling"])
        for actor in self.actors:
            actor.window.get_display.assert_called_once_with()
            actor.record.assert_called_once_with(
                "ready", pid=1234, same_display=True, native_wayland=True)
            actor.journal.close.assert_called_once_with()
        self.fixture.Gtk.main.assert_called_once_with()

    def test_rejects_x11_even_with_wayland_environment(self):
        self.display.__gtype__ = "GdkX11Display"
        with patch.dict(self.fixture.os.environ, {"GDK_BACKEND": "wayland"}), \
                self.assertRaisesRegex(RuntimeError, "native Wayland required"):
            self.run_fixture()
        self.assertEqual(self.actors, [])
        self.fixture.Gtk.main.assert_not_called()

    def test_rejects_no_display(self):
        self.fixture.Gdk.Display.get_default.return_value = None
        with self.assertRaisesRegex(RuntimeError, "native Wayland required"):
            self.run_fixture()
        self.fixture.GObject.type_name.assert_not_called()
        self.assertEqual(self.actors, [])

    def test_rejects_inexact_backend_type(self):
        self.display.__gtype__ = "OtherWaylandDisplay"
        with self.assertRaisesRegex(RuntimeError, "native Wayland required"):
            self.run_fixture()
        self.assertEqual(self.actors, [])

    def test_rejects_sibling_on_another_wayland_display(self):
        def different_display(name, directory):
            actor = self.actor(name, directory)
            if name == "Sibling":
                actor.window.get_display.return_value = Mock(__gtype__="GdkWaylandDisplay")
            return actor

        with self.assertRaisesRegex(RuntimeError, "siblings must share one Gdk.Display"):
            self.run_fixture(different_display)
        for actor in self.actors:
            actor.record.assert_not_called()
        self.fixture.Gtk.main.assert_not_called()


if __name__ == "__main__":
    unittest.main()
