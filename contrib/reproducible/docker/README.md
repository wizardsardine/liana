## Reproducible Liana builds

This repository contains the scripts to [reproducibly build](https://reproducible-builds.org/) Liana
on Windows and MacOS, for which we are missing [bootstrapable Guix builds](../guix/).

In order to perform the builds you will need [Docker](https://www.docker.com/).

The [`docker-build.sh`](./docker-build.sh) script will create a Docker image containing the build
environment for both Mac and Windows (see the respective `Dockerfile`s). It will then build the GUI
on Windows (the daemon isn't supported there) and both the daemon and the GUI on MacOS. The output
will be placed in a given `TARGET_DIR` (whose default value is `deter_build_target`).

### Build instructions

First of all, get [Docker](https://www.docker.com/).

Then get the MacOS SDK. It is required to be able to build the MacOS binaries. The `12_2` version is
required (`Xcode_12.2.xip`). You need to download it from Apple's website. An Apple ID and cookies
enabled for the hostname are required.  You can create one for free. (Note it is illegal to
distribute the archive.) Once logged in you can use the [direct
link](https://download.developer.apple.com/Developer_Tools/Xcode_12.2/Xcode_12.2.xip) to download
the archive. Alternatively, go to 'Downloads', then 'More' and search for [`Xcode
12.2`](https://developer.apple.com/download/all/?q=Xcode%2012.2).
The `sha256sum` of the downloaded XIP archive should be
`28d352f8c14a43d9b8a082ac6338dc173cb153f964c6e8fb6ba389e5be528bd0`.

Copy the downloaded `Xcode_12.2.xip` archive at the root of this repository (or provide a custom
path to the script by setting the `XCODE_PATH` env var).

Finally, run the script from the root of the repository:
```
./contrib/reproducible/docker/docker-build.sh
```
