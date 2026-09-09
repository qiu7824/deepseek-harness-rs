from __future__ import annotations

import base64
import io
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import patch

from PIL import Image

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import e2e_uu_desktop as probe


def capture(color=(0, 0, 0), *, image_format="JPEG", size=(12, 8)):
    encoded = io.BytesIO()
    Image.new("RGB", size, color).save(encoded, format=image_format)
    return {
        "state": {"interactive": True, "viewport": {"width": size[0], "height": size[1]}},
        "screenshot": {"mediaType": "image/jpeg", "base64": base64.b64encode(encoded.getvalue()).decode("ascii")},
    }


class UuCaptureEvidenceTests(unittest.TestCase):
    def test_black_and_other_uniform_images_are_valid_diagnostic_observations(self):
        _, black = probe.inspect_capture(capture())
        _, white = probe.inspect_capture(capture((255, 255, 255)))
        self.assertTrue(black["uniformity"]["uniformColor"])
        self.assertTrue(black["uniformity"]["allBlack"])
        self.assertTrue(white["uniformity"]["uniformColor"])
        self.assertFalse(white["uniformity"]["allBlack"])
        self.assertNotEqual(black["rgbSha256"], white["rgbSha256"])
        self.assertEqual(black["uniformity"]["rgbExtrema"], [[0, 0]] * 3)

    def test_jpeg_media_type_does_not_admit_a_different_image_format(self):
        with self.assertRaisesRegex(ValueError, "not a JPEG"):
            probe.inspect_capture(capture(image_format="PNG"))

    def test_empty_nonimage_invalid_base64_and_truncated_jpeg_are_rejected(self):
        fixture = capture()
        complete = base64.b64decode(fixture["screenshot"]["base64"])
        for encoded in ("", "%%%", base64.b64encode(b"not an image").decode(),
                        base64.b64encode(complete[:-30]).decode()):
            with self.subTest(encoded=encoded[:12]):
                fixture["screenshot"]["base64"] = encoded
                with self.assertRaises(ValueError):
                    probe.inspect_capture(fixture)

    def test_image_and_viewport_dimensions_must_agree(self):
        fixture = capture()
        fixture["state"]["viewport"]["width"] += 1
        with self.assertRaisesRegex(ValueError, "match the viewport"):
            probe.inspect_capture(fixture)
        fixture = capture()
        fixture["screenshot"]["width"] = 100
        with self.assertRaisesRegex(ValueError, "Screenshot dimensions"):
            probe.inspect_capture(fixture)

    def test_invalid_viewport_and_byte_budgets_fail_before_image_decode(self):
        for width, height in ((0, 8), (-1, 8), (True, 8), (1.5, 8), (1, None),
                              (probe.MAX_IMAGE_DIMENSION + 1, 1),
                              (probe.MAX_IMAGE_DIMENSION, probe.MAX_IMAGE_DIMENSION)):
            fixture = capture()
            fixture["state"]["viewport"] = {"width": width, "height": height}
            with self.subTest(width=width, height=height), patch.object(probe.Image, "open") as opened:
                with self.assertRaisesRegex(ValueError, "viewport"):
                    probe.inspect_capture(fixture)
                opened.assert_not_called()
        with patch.object(probe, "MAX_JPEG_BYTES", 16), patch.object(probe.Image, "open") as opened:
            with self.assertRaisesRegex(ValueError, "byte limit"):
                probe.inspect_capture(capture())
            opened.assert_not_called()

    def test_both_connections_save_three_samples_without_asserting_visual_success(self):
        calls = []
        fixture = capture()

        def act(action):
            calls.append(action)
            return fixture

        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory)
            initial = probe.capture_samples(act, output, "initial", sleep=lambda _: None)
            reconnected = probe.capture_samples(act, output, "reconnect", sleep=lambda _: None)
            report = probe.capture_evidence(initial, reconnected, {"status": "not-run", "decode": "not-run"})
            self.assertEqual(calls, ["capture"] * 6)
            self.assertEqual(len(list(output.glob("*.jpg"))), 6)
            self.assertTrue((output / "desktop.jpg").is_file())
            for sample in initial + reconnected:
                self.assertTrue(pathlib.Path(sample["image"]).is_file())
                self.assertTrue(sample["uniformity"]["allBlack"])
            self.assertEqual(report["status"], "partial")
            self.assertEqual(report["transport"]["status"], "passed")
            self.assertEqual(report["imageDecode"]["status"], "passed")
            self.assertEqual(report["visualCorrespondence"]["status"], "unverified")
            self.assertEqual(report["video"]["status"], "not-run")
            self.assertEqual(report["imageDecode"]["sourceFrameFreshness"], "unverified")
            self.assertFalse(report["desktopInputSent"])

    def test_insufficient_samples_cannot_be_published_as_complete_capture_evidence(self):
        with self.assertRaisesRegex(ValueError, "three decoded captures"):
            probe.capture_evidence([{}], [{}] * 3, {"status": "not-run"})

    def test_transport_only_video_never_implies_image_correspondence(self):
        _, image = probe.inspect_capture(capture((255, 255, 255)))
        report = probe.capture_evidence([image] * 3, [image] * 3,
                                        {"status": "transport-only", "transport": "passed", "decode": "unverified"})
        self.assertEqual(report["status"], "partial")
        self.assertEqual(report["video"]["decode"], "unverified")
        self.assertEqual(report["visualCorrespondence"]["status"], "unverified")


if __name__ == "__main__":
    unittest.main()
