# Liana reproducible builds

Releases of Liana are built in a reproducible manner, providing an assurance the binary a user is
going to run corresponds to the sources published. It enables the possibility for the user, or a
third party, to audit the code being ran.

For Linux binaries (and hopefully soon for all hosts) we go further and provide bootstrappable
builds, where the toolchain used to compile the source code reproducibly is itself built
reproducibly from source.

Learn more about reproducible builds [here](https://reproducible-builds.org/), and bootstrappable
builds [here](https://bootstrappable.org/).

For instructions on bootstrappable builds of Linux releases, see the [`guix`](./guix) folder.

For instructions on reproducible builds of Windows and MacOS releases, see the [`docker`](./docker)
folder.
