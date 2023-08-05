from shutil import copy as copyfile
from os.path import exists
from shutil import rmtree
from os import remove
from os import mkdir
from os import getcwd
import subprocess


stop = False
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
    if exists(f"{DIRNAME}.AppImage"):
        remove(f"{DIRNAME}.AppImage")

    mkdir(DIRNAME)
    mkdir(f"{DIRNAME}/Liana.AppDir")
    mkdir(f"{DIRNAME}/Liana.AppDir/usr")
    mkdir(f"{DIRNAME}/Liana.AppDir/usr/bin")

    copyfile('../gui/ui/static/logos/liana-app-icon.png', f"{DIRNAME}/Liana.AppDir/liana-app-icon.png")
    copyfile('../gui/target/release/liana-gui', f"{DIRNAME}/Liana.AppDir/usr/bin/liana-gui")

    file = open(f"{DIRNAME}/Liana.AppDir/Liana.desktop", "w")
    file.write('#!/usr/bin/env xdg-open\n')
    file.write('\n')
    file.write('[Desktop Entry]\n')
    file.write('Name=Liana\n')
    file.write('GenericName=Liana\n')
    file.write('Exec=liana-gui\n')
    file.write('Terminal=false\n')
    file.write('Type=Application\n')
    file.write('Icon=liana-app-icon\n')
    file.write('Categories=Finance\n')
    file.write('\n')
    
    file.close()
    
    file = open(f"{DIRNAME}/Liana.AppDir/AppRun", "w")
    file.write('#!/bin/sh\n')
    file.write('\n')
    file.write('HERE="$(dirname "$(readlink -f "${0}")")"\n')
    file.write('EXEC="${HERE}/usr/bin/liana-gui"\n')
    file.write('exec "${EXEC}"\n')
    
    file.close()
    
    print(f"{getcwd()=}")
    print(f"{getcwd()}/{DIRNAME}/Liana.AppDir/AppRun")
    subprocess.run(f"chmod +x {getcwd()}/{DIRNAME}/Liana.AppDir/AppRun", shell=True, check=True)
    subprocess.run(f"chmod +x {getcwd()}/{DIRNAME}/Liana.AppDir/Liana.desktop", shell=True, check=True)
    subprocess.run(f"ARCH=x86_64 appimagetool  {DIRNAME}/Liana.AppDir", shell=True, check=True)

    rmtree(DIRNAME)

