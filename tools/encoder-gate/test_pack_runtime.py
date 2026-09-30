"""Model-free archive-selection regression; native commands are not executed."""
import contextlib
import io
import pathlib
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch


class RuntimePackagingTest(unittest.TestCase):
    def test_nested_runtime_dependencies_are_packaged(self):
        main = runpy.run_path(str(pathlib.Path(__file__).with_name("pack-runtime.py")))["main"]
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            build, output = root / "build", root / "output"
            files = [
                f"libonnxruntime_{name}.a" for name in
                ("common", "flatbuffers", "framework", "graph", "lora", "mlas",
                 "optimizer", "providers", "session", "util")
            ] + [
                "model_package/libmodel_package.a",
                "_deps/re2-build/libre2.a",
                "_deps/protobuf-build/libprotobuf-lite.a",
                "_deps/protobuf-build/libprotobuf.a",
                "_deps/protobuf-build/libprotoc.a",
            ]
            for filename in files:
                path = build / filename
                path.parent.mkdir(parents=True, exist_ok=True)
                path.touch()
            with patch.object(sys, "argv", ["pack-runtime.py", str(build), str(output)]), \
                 patch("subprocess.run") as native, contextlib.redirect_stdout(io.StringIO()):
                main()
            inputs = (output / "archive-inputs.txt").read_text(encoding="utf-8").splitlines()
            self.assertIn("model_package/libmodel_package.a", inputs)
            self.assertIn("_deps/re2-build/libre2.a", inputs)
            self.assertIn("_deps/protobuf-build/libprotobuf-lite.a", inputs)
            self.assertNotIn("_deps/protobuf-build/libprotobuf.a", inputs)
            self.assertNotIn("_deps/protobuf-build/libprotoc.a", inputs)
            self.assertEqual(native.call_count, 2)


if __name__ == "__main__":
    unittest.main()
