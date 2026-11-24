import json
import os
import tempfile
import unittest

from ztile_ui import layout
from ztile_ui.store import AppSpec, Monitor, SavedLayout, Store


class LayoutTests(unittest.TestCase):
    def test_presets(self):
        for name, n in [("full", 1), ("2v", 2), ("3h", 3), ("2x2", 4), ("1+3", 4), ("2+1", 3)]:
            self.assertEqual(layout.cell_count(layout.preset(name)), n)
            self.assertEqual(len(layout.cells(layout.preset(name))), n)

    def test_reading_order(self):
        c = layout.cells(layout.preset("2x2"))
        self.assertEqual([x[:2] for x in c], [(0, 0), (0.5, 0), (0, 0.5), (0.5, 0.5)])

    def test_pixels_share_edges(self):
        px = layout.to_pixels(layout.cells(layout.preset("3v")), (0, 0, 1001, 700), 0)
        self.assertEqual(px[0][0] + px[0][2], px[1][0])
        self.assertEqual(px[2][0] + px[2][2], 1001)

    def test_weights_and_spec(self):
        n = layout.parse_spec("cols(60:c, 40:rows(c, c))")
        self.assertEqual(layout.to_spec(n), "cols(60:c, 40:rows(c, c))")
        self.assertAlmostEqual(layout.cells(n)[0][2], 0.6, places=5)

    def test_split_remove(self):
        n = layout.preset("2v")
        n = layout.split_cell(n, 1, "rows")
        self.assertEqual(layout.to_spec(n), "cols(c, rows(c, c))")
        n = layout.split_cell(n, 0, "cols")
        self.assertEqual(layout.to_spec(n), "cols(25:c, 25:c, 50:rows(c, c))")
        n = layout.remove_cell(n, 3)
        self.assertEqual(layout.to_spec(n), "cols(25:c, 25:c, 50:c)")
        self.assertIsNone(layout.remove_cell(layout.CELL, 0))
        self.assertEqual(layout.to_spec(layout.split_cell(layout.CELL, 0, "rows")), "rows(c, c)")
        two = layout.preset("2h")
        self.assertEqual(layout.remove_cell(two, 0), layout.CELL)

    def test_limits(self):
        self.assertIsNone(layout.parse_spec("rows(" * 10000 + "c" + ")" * 10000))
        self.assertIsNone(layout.parse_spec("cols(" + ",".join(["c"] * 17) + ")"))
        m = layout.parse_spec("cols(" + ",".join(["c"] * 16) + ")")
        self.assertIsNotNone(m)
        self.assertIsNone(layout.split_cell(m, 0, "rows"))
        deep = json.loads('{"dir":"cols","children":[' * 20 + '"cell"' + "]}" * 20)
        self.assertIsNone(layout.from_json(deep))
        n = layout.from_json({"dir": "cols", "sizes": [float("inf"), -5], "children": ["cell", "cell"]})
        self.assertTrue(all(c[2] >= c[0] for c in layout.cells(n)))

    def test_dividers(self):
        n = layout.preset("3v")
        d = layout.dividers(n)[0]
        layout.move_divider(n, d, 0.5)
        c = layout.cells(n)
        self.assertAlmostEqual(c[0][2], 0.5, places=4)
        self.assertAlmostEqual(c[1][2], 2 / 3, places=4)
        layout.move_divider(n, layout.dividers(n)[0], 0.99)
        c = layout.cells(n)
        self.assertGreaterEqual(c[1][2] - c[1][0], 0.049)

    def test_json_roundtrip_matches_windows_format(self):
        for p in layout.PRESETS:
            n = layout.preset(p)
            self.assertEqual(layout.from_json(json.loads(json.dumps(layout.to_json(n)))), n)


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        os.environ["XDG_CONFIG_HOME"] = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def test_roundtrip(self):
        s = Store()
        s.put(SavedLayout("Chat", Monitor("eDP-1", 2560, 1600), layout.preset("2x2"), 4, False, True,
                          [AppSpec("discord", "Discord"), AppSpec("brave-x-Default", "WhatsApp", "Chat \"x\""), None, None]))
        s.save()
        back = Store.load()
        self.assertEqual(back.layouts, s.layouts)

    def test_put_replaces_case_insensitive(self):
        s = Store()
        s.put(SavedLayout("Work"))
        s.put(SavedLayout("İş"))
        s.put(SavedLayout("WORK", gap=8))
        self.assertEqual(len(s.layouts), 2)
        self.assertEqual(s.get("work").gap, 8)
        self.assertTrue(s.remove("Work"))

    def test_corrupt_file_moved_aside(self):
        p = Store.path()
        p.parent.mkdir(parents=True)
        p.write_text("{not json")
        self.assertEqual(Store.load().layouts, [])
        self.assertTrue(p.with_suffix(".json.bad").exists())

    def test_windows_file_is_readable(self):
        p = Store.path()
        p.parent.mkdir(parents=True)
        p.write_text(json.dumps({"version": 1, "layouts": [
            {"name": "A", "monitor": {"device": "\\\\.\\DISPLAY1", "width": 1920, "height": 1080},
             "tree": "cell", "apps": [{"exe": "C:\\x.exe"}]}]}))
        l = Store.load().layouts[0]
        self.assertEqual(l.monitor.width, 1920)
        self.assertEqual(l.apps, [None])  # Windows app entries don't apply here
        self.assertFalse(l.keep_changes)


if __name__ == "__main__":
    unittest.main()
