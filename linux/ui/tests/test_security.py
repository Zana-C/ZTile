import os
import stat
import tempfile
import unittest
from pathlib import Path

from ztile_ui.files import desktop_value, exec_arg, write_atomic
from ztile_ui.store import AppSpec, SavedLayout, Store


class FileSafety(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_desktop_value_cannot_add_keys(self):
        v = desktop_value("Work\nExec=rm -rf ~\r\x00")
        self.assertNotIn("\n", v)
        self.assertNotIn("\r", v)
        self.assertNotIn("\x00", v)
        self.assertEqual(desktop_value("a\\b"), "a\\\\b")

    def test_exec_arg_quotes(self):
        self.assertEqual(exec_arg("/usr/bin/ztile"), "/usr/bin/ztile")
        self.assertEqual(exec_arg("/home/a b/ztile"), '"/home/a b/ztile"')
        q = exec_arg('/x/$HOME/`id`/"q"')
        self.assertTrue(q.startswith('"') and q.endswith('"'))
        self.assertNotIn("$H", q.replace("\\\\$H", ""))

    def test_write_atomic_replaces_symlink_not_target(self):
        victim = self.dir / "victim"
        victim.write_text("keep")
        link = self.dir / "out.desktop"
        link.symlink_to(victim)
        write_atomic(link, "new")
        self.assertEqual(victim.read_text(), "keep")
        self.assertFalse(link.is_symlink())
        self.assertEqual(link.read_text(), "new")

    def test_layouts_file_is_private(self):
        os.environ["XDG_CONFIG_HOME"] = str(self.dir)
        Store([SavedLayout("A", apps=[AppSpec("x", "X", "secret chat")])]).save()
        mode = stat.S_IMODE(Store.path().stat().st_mode)
        self.assertEqual(mode, 0o600)
        self.assertEqual(stat.S_IMODE(Store.path().parent.stat().st_mode), 0o700)

    def test_untrusted_layout_values_are_bounded(self):
        l = SavedLayout.from_json({"name": "x" * 5000, "tree": "cell", "gap": 1e308,
                                   "apps": [{"app": "a" * 5000, "window": 5}]})
        self.assertLessEqual(len(l.name), 60)
        self.assertEqual(l.gap, 64)
        self.assertLessEqual(len(l.apps[0].app), 256)
        self.assertEqual(l.apps[0].window, "")
        self.assertIsNone(SavedLayout.from_json({"name": "x", "tree": {"dir": "evil"}}))


if __name__ == "__main__":
    unittest.main()
