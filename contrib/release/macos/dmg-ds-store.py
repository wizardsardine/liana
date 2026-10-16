#!/usr/bin/env python3
"""Write a .DS_Store that lays out a DMG's Finder window (icon view, icon
positions, optional background picture). Runs on Linux: no macOS APIs used.

usage: dmg-ds-store.py <out .DS_Store> <App.app> <volume name>
                       [--background <png path relative to volume root>]

Requires the `ds_store` and `mac_alias` packages
(nixpkgs: python3Packages.ds-store pulls both).
"""

import argparse
import datetime

from ds_store import DSStore
from mac_alias import Alias, VolumeInfo, TargetInfo, ALIAS_FIXED_DISK, ALIAS_KIND_FILE

p = argparse.ArgumentParser()
p.add_argument("out")
p.add_argument("app")
p.add_argument("volname")
p.add_argument("--background", default=None)
p.add_argument("--window", default="400,100,500,380", help="x,y,w,h")
args = p.parse_args()

x, y, w, h = (int(v) for v in args.window.split(","))


def background_alias(volname, rel_path):
    """Build an alias record by hand so it can be produced without mounting
    the volume. Finder resolves it by volume name + path."""
    now = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
    folder = rel_path.rsplit("/", 1)[0] if "/" in rel_path else volname
    volume = VolumeInfo(
        name=volname,
        creation_date=now,
        fs_type=b"H+",
        disk_type=ALIAS_FIXED_DISK,
        attribute_flags=0,
        fs_id=b"\0\0",
        posix_path="/Volumes/" + volname,
    )
    target = TargetInfo(
        kind=ALIAS_KIND_FILE,
        filename=rel_path.rsplit("/", 1)[-1],
        folder_cnid=0,
        cnid=0,
        creation_date=now,
        creator_code=b"\0\0\0\0",
        type_code=b"\0\0\0\0",
        folder_name=folder,
        carbon_path=volname + ":" + rel_path.replace("/", ":"),
        posix_path="/" + rel_path,
    )
    return Alias(volume=volume, target=target).to_bytes()


with DSStore.open(args.out, "w+") as d:
    d["."]["vSrn"] = ("long", 1)
    d["."]["bwsp"] = {
        "ShowStatusBar": False,
        "WindowBounds": "{{%d, %d}, {%d, %d}}" % (x, y, w, h),
        "ContainerShowSidebar": False,
        "PreviewPaneVisibility": False,
        "SidebarWidth": 0,
        "ShowTabView": False,
        "ShowToolbar": False,
        "ShowPathbar": False,
        "ShowSidebar": False,
    }
    icvp = {
        "viewOptionsVersion": 1,
        "backgroundType": 0,
        "backgroundColorRed": 1.0,
        "backgroundColorGreen": 1.0,
        "backgroundColorBlue": 1.0,
        "gridOffsetX": 0.0,
        "gridOffsetY": 0.0,
        "gridSpacing": 100.0,
        "arrangeBy": "none",
        "showIconPreview": True,
        "showItemInfo": False,
        "labelOnBottom": True,
        "textSize": 12.0,
        "iconSize": 128.0,
        "scrollPositionX": 0.0,
        "scrollPositionY": 0.0,
    }
    if args.background:
        icvp["backgroundType"] = 2  # picture
        icvp["backgroundImageAlias"] = background_alias(args.volname, args.background)
    d["."]["icvp"] = icvp
    d[args.app]["Iloc"] = (w // 4, h // 2 - 30)
    d["Applications"]["Iloc"] = (3 * w // 4, h // 2 - 30)
