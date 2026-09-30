"""Model-free desktop archive regression; no native commands are executed."""
import contextlib
import io
from pathlib import Path
import runpy
import tempfile
import unittest
from unittest.mock import patch


class DesktopPackagingTest(unittest.TestCase):
    def test_windows_protobuf_lite_does_not_include_full_or_generator(self):
        main = runpy.run_path(str(Path(__file__).with_name("desktop-runtime.py")))["main"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            build = root / "ort/Release"
            names = ["onnxruntime_common.lib", "onnxruntime_framework.lib", "onnxruntime_session.lib",
                     "onnxruntime_providers.lib", "libre2.lib", "libmodel_package.lib",
                     "libprotobuf-lite.lib", "libprotobuf.lib", "libprotoc.lib"]
            for name in names:
                path = build / "nested" / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.touch()
            with patch.dict(main.__globals__, NATIVE=root, SOURCE=root / "source", BUILD=root / "ort",
                            PACKED=root / "packed", WINDOWS=True), patch("subprocess.run"), \
                    contextlib.redirect_stdout(io.StringIO()):
                main()
            packed = (root / "packed/archive-inputs.txt").read_text()
            self.assertIn("libprotobuf-lite.lib", packed)
            self.assertIn("libre2.lib", packed)
            self.assertNotIn("/libprotobuf.lib", packed)
            self.assertNotIn("/libprotoc.lib", packed)


class DesktopDependencyTest(unittest.TestCase):
    def test_windows_system_libraries_are_not_runtime_sidecars(self):
        verify = runpy.run_path(str(Path(__file__).with_name("desktop-smoke.py")))["verify_windows_dependencies"]
        verify("    KERNEL32.dll\n    bcryptPrimitives.dll\n    SETUPAPI.dll\n    dxgi.dll\n")

    def test_native_runtime_and_crt_sidecars_are_rejected(self):
        verify = runpy.run_path(str(Path(__file__).with_name("desktop-smoke.py")))["verify_windows_dependencies"]
        for name in ["onnxruntime.dll", "vcruntime140.dll", "msvcp140.dll", "libprotobuf.dll"]:
            with self.subTest(library=name), self.assertRaises(RuntimeError):
                verify(f"    KERNEL32.dll\n    {name}\n")


if __name__ == "__main__":
    unittest.main()
