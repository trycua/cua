import tempfile
import unittest
from pathlib import Path

from snapshot_complexity import code_and_complexity, production_source


class ProductionSourceTests(unittest.TestCase):
    def test_generic_async_functions_and_nested_decisions_are_counted(self):
        source = '''
impl<T> Cache<T> {
    async fn outer(x: bool) -> Option<()> {
        fn inner() { if true {} }
        if x && x { Some(())?; }
        let Some(()) = Some(()) else { return None; };
        for _ in 0..2 {}
        while x {}
        loop { break; }
        match 1 { 0 => (), n if n > 1 => (), _ => () };
        Some(())
    }
}
'''
        _, functions, error = code_and_complexity(source)
        self.assertFalse(error)
        self.assertEqual([function["ccn"] for function in functions], [11, 2])

    def test_test_only_items_and_external_modules_are_excluded(self):
        source = '''
fn production() { if true {} }
#[cfg(test)]
mod tests { fn hidden() { if true {} } }
#[cfg(all(test, unix))]
mod native_tests { fn hidden_native() {} }
#[cfg(all(test, target_os = "windows"))]
#[path = "native_fixture.rs"]
mod fixture;
#[tokio::test(flavor = "multi_thread")]
async fn hidden_async() {}
#[cfg(any(test, target_os = "windows"))]
fn production_on_windows() {}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "sample.rs"
            path.write_text(source)
            result, external = production_source(path)
            self.assertIn("fn production()", result)
            self.assertIn("fn production_on_windows()", result)
            self.assertNotIn("hidden", result)
            self.assertEqual(external, [path.parent / "native_fixture.rs"])
            self.assertEqual(result.count("\n"), source.count("\n"))


if __name__ == "__main__":
    unittest.main()
