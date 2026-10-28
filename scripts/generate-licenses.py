#!/usr/bin/env python3
"""Rebuild the embedded Windows GUI license inventory without network access."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "crates/vivid-gui/assets/third-party-licenses.json"
TARGET = "x86_64-pc-windows-msvc"
LICENSE_NAME = re.compile(
    r"^(licen[cs]e|copying|notice|copyright|unlicen[cs]e)([._-]|$)", re.I
)
APACHE_FALLBACKS = {
    "accesskit", "accesskit_consumer", "accesskit_windows", "accesskit_winit",
    "ecolor", "eframe", "egui", "egui-wgpu", "egui-winit", "emath", "epaint",
    "epaint_default_fonts", "gl_generator", "khronos_api", "profiling", "spirv",
}
FONT_FILES = (
    "fonts/emoji-icon-font-mit-license.txt", "fonts/Hack-Regular.txt",
    "fonts/OFL.txt", "fonts/UFL.txt",
)
ACCESSKIT_COMMIT = "a55d3e1a18bb9ef0e4bccc9083fb13c3e0ad8969"
ACCESSKIT_CHROMIUM_SHA256 = "845022e0c1db1abb41a6ba4cd3c4b674ec290f3359d9d3c78ae558d4c0ed9308"


def read_text(path):
    text = path.read_text(encoding="utf-8-sig")
    if not text.strip() or "\0" in text:
        raise ValueError(f"Empty or non-text license: {path}")
    return text.replace("\r\n", "\n").strip()


def cargo(*args):
    environment = os.environ.copy()
    environment.setdefault("CARGO_TARGET_DIR", str(ROOT / "target"))
    result = subprocess.run(
        ["cargo", *args], cwd=ROOT, env=environment, check=True,
        encoding="utf-8", stdout=subprocess.PIPE,
    )
    return result.stdout


def dependencies():
    metadata = json.loads(cargo(
        "metadata", "--locked", "--offline", "--format-version", "1",
        "--filter-platform", TARGET,
    ))
    tree = cargo(
        "tree", "--locked", "--offline", "--target", TARGET, "-p", "vivid-gui",
        "-e", "normal,build", "--prefix", "none", "--format", "{p}",
    )
    selected = set(re.findall(r"^([\w-]+) v([^\s]+)", tree, re.M))
    packages = [
        package for package in metadata["packages"]
        if (package["name"], package["version"]) in selected
        and package["id"] not in metadata["workspace_members"]
    ]
    actual = {(p["name"], p["version"]) for p in packages}
    local = {
        (p["name"], p["version"]) for p in metadata["packages"]
        if p["id"] in metadata["workspace_members"]
    }
    if actual != selected - local or len(actual) != len(packages):
        raise ValueError("Cargo tree cannot be matched unambiguously to metadata")
    return sorted(packages, key=lambda p: (p["name"].lower(), p["version"]))


def root_of(package):
    return Path(package["manifest_path"]).parent


def section(label, text):
    return f"===== {label} =====\n\n{text}"


def package_file(packages, name, filename):
    matches = [p for p in packages if p["name"] == name]
    if len(matches) != 1:
        raise ValueError(f"Expected one license source package: {name}")
    package = matches[0]
    label = f"{package['name']} {package['version']}/{filename}"
    return label, read_text(root_of(package) / filename)


def source_notices(root):
    notices = []
    for path in sorted(root.rglob("*.rs")):
        lines = path.read_text(encoding="utf-8-sig").splitlines()
        header = []
        for line in lines:
            if not line.strip() or line.lstrip().startswith("//"):
                header.append(line)
            else:
                break
        text = "\n".join(header).strip()
        if re.search(r"copyright", text, re.I) and text not in [t for _, t in notices]:
            notices.append((path.relative_to(root).as_posix() + " source notice", text))
    return notices


def license_record(package, packages, warnings):
    name = package["name"]
    expression = package["license"]
    if not expression:
        raise ValueError(f"No SPDX license declaration: {name}")
    root = root_of(package)
    paths = {
        path for path in root.rglob("*")
        if path.is_file() and LICENSE_NAME.match(path.name)
    }
    if package["license_file"]:
        paths.add(root / package["license_file"])
    if name == "epaint_default_fonts":
        paths.update(root / filename for filename in FONT_FILES)
    parts = [
        section(path.relative_to(root).as_posix(), read_text(path))
        for path in sorted(paths, key=lambda p: p.relative_to(root).as_posix())
    ]
    missing_main = not any(path.parent == root for path in paths)
    if missing_main:
        declaration = (
            f"The upstream Cargo.toml declares: {expression}.\n"
            f"Upstream repository: {package['repository']}\n"
            "The published crate omits its root license file. The following "
            "license is selected from that declaration; its complete standard "
            "text is reproduced from the explicitly identified local source. "
            "No other package's copyright is attributed to this package."
        )
        if name in APACHE_FALLBACKS and "Apache-2.0" in expression:
            label, text = package_file(packages, "ahash", "LICENSE-APACHE")
            if "Version 2.0, January 2004" not in text:
                raise ValueError("Apache fallback source is not Apache License 2.0")
        elif name == "clipboard-win" and expression == "BSL-1.0":
            label, text = package_file(packages, "error-code", "LICENSE")
            if not text.startswith("Boost Software License - Version 1.0"):
                raise ValueError("Boost fallback source is not BSL-1.0")
        elif name == "pelite-macros" and expression == "MIT":
            source = next(p for p in packages if p["name"] == "pelite")
            if source["repository"] != package["repository"]:
                raise ValueError("pelite license source repository mismatch")
            label, text = package_file(packages, "pelite", "license.txt")
            declaration = (
                f"Upstream Cargo.toml declares: {expression}.\n"
                f"Upstream repository: {package['repository']}\n"
                "The crate omits its license file. The complete license below "
                "comes from pelite, published from the same upstream repository."
            )
        else:
            raise ValueError(f"No reviewed complete license source: {name} {expression}")
        parts.insert(0, declaration)
        parts.append(section(label, text))
        parts.extend(section(label, text) for label, text in source_notices(root))
        warnings.append(f"{name}: root license absent; using {label}")
    if name == "accesskit":
        vcs = json.loads(read_text(root / ".cargo_vcs_info.json"))
        if package["version"] != "0.24.1" or vcs["git"]["sha1"] != ACCESSKIT_COMMIT:
            raise ValueError("AccessKit version or source commit changed; review Chromium license")
        path = ROOT / "scripts/licenses/accesskit-0.24.1-LICENSE.chromium"
        content = path.read_bytes().replace(b"\r\n", b"\n")
        if hashlib.sha256(content).hexdigest() != ACCESSKIT_CHROMIUM_SHA256:
            raise ValueError("Pinned AccessKit Chromium license text differs from reviewed source")
        source_url = (
            "https://raw.githubusercontent.com/AccessKit/accesskit/"
            f"{ACCESSKIT_COMMIT}/LICENSE.chromium"
        )
        parts.append(section(
            f"LICENSE.chromium (upstream source commit {ACCESSKIT_COMMIT})",
            f"Source: {source_url}\nSHA-256: {ACCESSKIT_CHROMIUM_SHA256}\n\n"
            + content.decode("utf-8").strip(),
        ))
        expression = f"({expression}) AND BSD-3-Clause"
    if not parts:
        raise ValueError(f"No complete license text: {name}")
    return {
        "name": name, "version": package["version"], "license": expression,
        "text": "\n\n".join(parts),
    }


def assets():
    gui = ROOT / "crates/vivid-gui/assets"
    return [
        {
            "name": "Resource Han Rounded CN Medium", "version": "0.990",
            "license": "OFL-1.1",
            "text": "Source: https://github.com/CyanoHao/Resource-Han-Rounded\n\n"
            + section("crates/vivid-gui/assets/OFL.txt", read_text(gui / "OFL.txt")),
        },
        {
            "name": "RHI / ReShade embedded configuration templates",
            "version": "2026-08-31 source snapshot", "license": "GPL-3.0-only",
            "text": (
                "Source: https://github.com/RankFTW/RHI\n"
                "ReShade attribution: https://github.com/crosire/reshade\n"
                "Embedded text only: crates/vivid-core/assets/reshade.gta5.ini, "
                "reshade.rdr2.ini and ReShade64.json. The historical filenames "
                "identify the bundled snapshot, not a current upstream release.\n"
                "The JSON manifest describes crosire's ReShade Vulkan layer; "
                "no ReShade implementation or runtime binary is redistributed.\n\n"
            ) + section("LICENSE (GPL version 3)", read_text(ROOT / "LICENSE")),
        },
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Fail if the inventory differs")
    args = parser.parse_args()
    packages = dependencies()
    warnings = []
    embedded = assets() + json.loads(read_text(ROOT / "crates/vivid-gui/assets/social-icons/licenses.json"))
    records = [license_record(p, packages, warnings) for p in packages] + embedded
    records.sort(key=lambda r: (r["name"].lower(), r["version"]))
    data = (json.dumps(records, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    if args.check:
        if not OUTPUT.exists() or OUTPUT.read_bytes() != data:
            print("License inventory is stale; run python scripts/generate-licenses.py", file=sys.stderr)
            return 1
    else:
        OUTPUT.write_bytes(data)
    print(f"{'Checked' if args.check else 'Generated'} {len(records)} entries: "
          f"{len(packages)} Windows GUI normal/build dependencies + {len(embedded)} embedded assets")
    for warning in warnings:
        print(f"NOTICE: {warning}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"License generation failed: {error}", file=sys.stderr)
        sys.exit(1)
