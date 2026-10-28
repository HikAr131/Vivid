"""Verify original, embedded and rendered poster QR payloads without exposing them."""

import argparse
import hashlib
import json
from pathlib import Path

from PIL import Image
import zxingcpp

ROOT = Path(__file__).resolve().parents[1]


def decode(path):
    image = Image.open(path)
    values = sorted({result.bytes for result in zxingcpp.read_barcodes(image)
                     if result.valid and result.format == zxingcpp.BarcodeFormat.QRCode})
    if len(values) != 2:
        raise ValueError(f"Expected exactly two QR payloads: {path}, found {len(values)}")
    return (image.height, image.width), values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("original", type=Path)
    parser.add_argument("--rendered", type=Path, action="append", default=[])
    args = parser.parse_args()
    embedded = ROOT / "crates/vivid-gui/assets/donate-poster.png"
    if args.original.read_bytes() != embedded.read_bytes():
        raise ValueError("Embedded PNG differs from the original")
    dimensions, original_values = decode(args.original)
    if dimensions != (1536, 1024):
        raise ValueError(f"Unexpected original dimensions: {dimensions}")
    evidence = {"original_sha256": hashlib.sha256(embedded.read_bytes()).hexdigest(),
                "size": [1024, 1536], "codes": len(original_values),
                "payload_sha256": [hashlib.sha256(v).hexdigest() for v in original_values],
                "rendered": []}
    for path in [embedded, *args.rendered]:
        _, values = decode(path)
        if values != original_values:
            raise ValueError(f"QR contents differ from original: {path}")
        evidence["rendered"].append({"file": path.name, "matches": True})
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
