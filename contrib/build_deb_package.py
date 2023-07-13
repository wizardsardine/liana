from shutil import copy as copyfile
from os.path import exists
from os import scandir
from shutil import rmtree
from os import remove
from os import mkdir
from os import chmod
from math import ceil
import subprocess


def get_size(path) -> int:
    total = 0
    with scandir(path) as it:
        for entry in it:
            if entry.is_file():
                total += entry.stat().st_size
            elif entry.is_dir():
                total += get_size(entry.path)
    return total

stop = False
if not exists('../target/release/lianad'):
    print('Cannot find lianad binaries in target/release/')
    stop = True

if not exists('../target/release/liana-cli'):
    print('Cannot find liana-cli binaries in target/release/')
    stop = True

if not exists('../gui/target/release/liana-gui'):
    print('Cannot find liana-gui binaries in gui/target/release/')
    stop = True

if not stop:

    print("Liana version:")
    version = float(input())

    arch = subprocess.run("dpkg-architecture -q DEB_TARGET_ARCH", shell=True,
                          stdout=subprocess.PIPE).stdout.decode('utf-8')[:-1]
    arch_multi = subprocess.run("dpkg-architecture -q DEB_TARGET_MULTIARCH", shell=True,
                                stdout=subprocess.PIPE).stdout.decode('utf-8')[:-1]


    DIRNAME = f"liana-{version}-{arch_multi}"

    if exists(DIRNAME):
        rmtree(DIRNAME)
    if exists(f"{DIRNAME}.deb"):
        remove(f"{DIRNAME}.deb")

    mkdir(DIRNAME)
    mkdir(f"{DIRNAME}/DEBIAN")
    mkdir(f"{DIRNAME}/opt")
    mkdir(f"{DIRNAME}/opt/liana")
    mkdir(f"{DIRNAME}/usr")
    mkdir(f"{DIRNAME}/usr/bin")
    mkdir(f"{DIRNAME}/usr/share")
    mkdir(f"{DIRNAME}/usr/share/applications")

    copyfile('../gui/ui/static/logos/liana-app-icon.png', f"{DIRNAME}/opt/liana/liana-app-icon.png")
    copyfile('../target/release/lianad', f"{DIRNAME}/usr/bin/lianad")
    copyfile('../target/release/liana-cli', f"{DIRNAME}/usr/bin/liana-cli")
    copyfile('../gui/target/release/liana-gui', f"{DIRNAME}/usr/bin/liana-gui")


    file = open(f"{DIRNAME}/usr/share/applications/Liana.desktop", "w")
    lines = """#!/usr/bin/env xdg-open
        
        [Desktop Entry]
        Name=Liana
        GenericName=Liana
        Exec=/usr/bin/liana-gui
        Terminal=False
        Type=Application
        Icon=/opt/liana/liana-app-icon.png
        Categories=Finance;Network;"""

    file.write(lines)
    file.close()

    size = ceil(get_size(DIRNAME)/1000) # kB

    file = open(f"{DIRNAME}/DEBIAN/control", "w")
    lines = f"Package: Liana\n"
    lines += f"Version: {str(version)}\n"
    lines += f"Architecture: {arch}\n"
    lines += "Essential: no\n"
    lines += f"Installed-Size: {size}\n"
    lines += "Depends: udev, libfontconfig1-dev,libudev-dev, libc6 (>= 2.34)\n"
    lines += "Priority: optionnal\n"
    lines += f"Breaks: Liana (<< {str(version)})\n"
    lines += f"Replaces: Liana (<< {str(version)})\n"
    lines += """Maintainer: A. Poinsot <darosior@protonmail.com>
Homepage: https://www.wizardsardine.com/
Description: Liana is a Bitcoin wallet
"""

    file.write(lines)
    file.close()

    subprocess.run(f"dpkg-deb --build {DIRNAME}", shell=True, check=True)

    rmtree(DIRNAME)

