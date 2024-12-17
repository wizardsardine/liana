(use-modules
  (gnu packages llvm)
  (gnu packages rust)
  (gnu packages base)
  (gnu packages crates-io)
  (guix build-system cargo)
  ((guix licenses) #:prefix license:)
  (guix download)
  (guix packages)
  (guix utils))

(define-public rust-goblin-0.9
  (package
    (name "rust-goblin")
    (version "0.9.2")
    (source (origin
              (method url-fetch)
              (uri (crate-uri "goblin" version))
              (file-name (string-append name "-" version ".tar.gz"))
              (sha256
               (base32
                "08yrnjj5j4nddh6y1r8kf35ys7p3iwg6npga3nc4cwfps4r3zask"))))
    (build-system cargo-build-system)
    (arguments
     `(#:cargo-inputs (("rust-log" ,rust-log-0.4)
                       ("rust-plain" ,rust-plain-0.2)
                       ("rust-scroll" ,rust-scroll-0.12))
       #:cargo-development-inputs (("rust-stderrlog" ,rust-stderrlog-0.5))))
    (home-page "https://github.com/m4b/goblin")
    (synopsis
     "An impish, cross-platform, ELF, Mach-o, and PE binary parsing and loading crate")
    (description
     "An impish, cross-platform, ELF, Mach-o, and PE binary parsing and loading crate")
    (license license:expat)))

(define-public rust-fat-macho-0.4
  (package
    (name "rust-fat-macho")
    (version "0.4.9")
    (source (origin
              (method url-fetch)
              (uri (crate-uri "fat-macho" version))
              (file-name (string-append name "-" version ".tar.gz"))
              (sha256
               (base32
                "0idkn366wipv2l757yqfgzgibqc6jvm89gdk9kpgmvf6lv54b72c"))))
    (build-system cargo-build-system)
    (arguments
     `(#:cargo-inputs (("rust-goblin" ,rust-goblin-0.9)
                       ("rust-llvm-bitcode" ,rust-llvm-bitcode-0.1))))
    (home-page "https://github.com/messense/fat-macho-rs.git")
    (synopsis "Mach-O Fat Binary Reader and Writer")
    (description "Mach-O Fat Binary Reader and Writer")
    (license license:expat)))

(define-public rust-rustflags-0.1
  (package
    (name "rust-rustflags")
    (version "0.1.6")
    (source (origin
              (method url-fetch)
              (uri (crate-uri "rustflags" version))
              (file-name (string-append name "-" version ".tar.gz"))
              (sha256
               (base32
                "1h1al0xhd9kzy8q8lzw6rxip5zjifxigfrm3blf462mmkwar5z6p"))))
    (build-system cargo-build-system)
    (arguments
     `(#:cargo-development-inputs (("rust-cmake" ,rust-cmake-0.1))))
    (home-page "https://github.com/dtolnay/rustflags")
    (synopsis "Parser for CARGO_ENCODED_RUSTFLAGS")
    (description "Parser for CARGO_ENCODED_RUSTFLAGS")
    (license (list license:expat license:asl2.0))))

(define-public rust-cargo-config2-0.1
  (package
    (name "rust-cargo-config2")
    (version "0.1.29")
    (source
     (origin
       (method url-fetch)
       (uri (crate-uri "cargo-config2" version))
       (file-name (string-append name "-" version ".tar.gz"))
       (sha256
        (base32 "15cmkvxad1b33bvx2d1h7bnjlbvq1lpyi5jybk0jq9mrxi5ha90i"))))
    (build-system cargo-build-system)
    (arguments
     `(#:cargo-inputs (("rust-home" ,rust-home-0.5)
                       ("rust-serde" ,rust-serde-1)
                       ("rust-serde-derive" ,rust-serde-derive-1)
                       ("rust-toml-edit" ,rust-toml-edit-0.22))
       #:cargo-development-inputs (("rust-anyhow" ,rust-anyhow-1)
                                   ("rust-build-context" ,rust-build-context-0.1)
                                   ("rust-clap" ,rust-clap-4)
                                   ("rust-duct" ,rust-duct-0.13)
                                   ("rust-fs-err" ,rust-fs-err-2)
                                   ("rust-lexopt" ,rust-lexopt-0.3)
                                   ("rust-rustversion" ,rust-rustversion-1)
                                   ("rust-serde-json" ,rust-serde-json-1)
                                   ("rust-shell-escape" ,rust-shell-escape-0.1)
                                   ("rust-static-assertions" ,rust-static-assertions-1)
                                   ("rust-tempfile" ,rust-tempfile-3)
                                   ("rust-toml" ,rust-toml-0.8))))
    (home-page "https://github.com/taiki-e/cargo-config2")
    (synopsis "Load and resolve Cargo configuration.")
    (description "This package provides Load and resolve Cargo configuration.")
    (license (list license:asl2.0 license:expat))))

(define-public rust-cargo-options-0.7
  (package
    (name "rust-cargo-options")
    (version "0.7.4")
    (source
     (origin
       (method url-fetch)
       (uri (crate-uri "cargo-options" version))
       (file-name (string-append name "-" version ".tar.gz"))
       (sha256
        (base32 "07rj2hkqyan8c2jmvfa35ji6wy0pqs6j7k2a6bmpcym3q13h4m7k"))))
    (build-system cargo-build-system)
    (arguments
     `(#:cargo-inputs (("rust-anstyle" ,rust-anstyle-1)
                       ("rust-clap" ,rust-clap-4))
       #:cargo-development-inputs (("rust-trycmd" ,rust-trycmd-0.14))))
    (home-page "https://github.com/messense/cargo-options")
    (synopsis "Reusable common Cargo command line options")
    (description
     "This package provides Reusable common Cargo command line options.")
    (license license:expat)))

(define-public rust-crc-3
  (package
    (name "rust-crc")
    (version "3.2.1")
    (source
     (origin
       (method url-fetch)
       (uri (crate-uri "crc" version))
       (file-name (string-append name "-" version ".tar.gz"))
       (sha256
        (base32 "0dnn23x68qakzc429s1y9k9y3g8fn5v9jwi63jcz151sngby9rk9"))))
    (build-system cargo-build-system)
    (arguments
     `(#:cargo-inputs (("rust-crc-catalog" ,rust-crc-catalog-2))
       #:cargo-development-inputs (("rust-criterion" ,rust-criterion-0.4))))
    (home-page "https://github.com/mrhooray/crc-rs.git")
    (synopsis "Rust implementation of CRC with support of various standards")
    (description
     "This package provides Rust implementation of CRC with support of various standards.")
    (license (list license:expat license:asl2.0))))

(define-public rust-target-lexicon-0.12
  (package
    (name "rust-target-lexicon")
    (version "0.12.16")
    (source
     (origin
       (method url-fetch)
       (uri (crate-uri "target-lexicon" version))
       (file-name (string-append name "-" version ".tar.gz"))
       (sha256
        (base32 "1cg3bnx1gdkdr5hac1hzxy64fhw4g7dqkd0n3dxy5lfngpr1mi31"))))
    (build-system cargo-build-system)
    (arguments
     `(#:cargo-inputs (("rust-serde" ,rust-serde-1))
       #:cargo-development-inputs (("rust-serde-json" ,rust-serde-json-1))))
    (home-page "https://github.com/bytecodealliance/target-lexicon")
    (synopsis "Targeting utilities for compilers and related tools")
    (description
     "This package provides Targeting utilities for compilers and related tools.")
    (license license:asl2.0)))


(define-public rust-cargo-zigbuild-0.19
  (package
    (name "rust-cargo-zigbuild")
    (version "0.19.4")
    (source
     (origin
       (method url-fetch)
       (uri (crate-uri "cargo-zigbuild" version))
       (file-name (string-append name "-" version ".tar.gz"))
       (sha256
        (base32 "0r4pdjvj7iz928gyyj569y7rwaz9mwddaz0s0a0cggiybmandpi0"))))
    (build-system cargo-build-system)
    (arguments
     `(#:cargo-inputs (("rust-anyhow" ,rust-anyhow-1)
                       ("rust-cargo-config2" ,rust-cargo-config2-0.1)
                       ("rust-cargo-options" ,rust-cargo-options-0.7)
                       ("rust-cargo-metadata" ,rust-cargo-metadata-0.18)
                       ("rust-clap" ,rust-clap-4)
                       ("rust-crc" ,rust-crc-3)
                       ("rust-dirs" ,rust-dirs-5)
                       ("rust-fat-macho" ,rust-fat-macho-0.4)
                       ("rust-fs-err" ,rust-fs-err-2)
                       ("rust-path-slash" ,rust-path-slash-0.2)
                       ("rust-rustc-version" ,rust-rustc-version-0.4)
                       ("rust-rustflags" ,rust-rustflags-0.1)
                       ("rust-semver" ,rust-semver-1)
                       ("rust-serde" ,rust-serde-1)
                       ("rust-serde-json" ,rust-serde-json-1)
                       ("rust-shlex" ,rust-shlex-1)
                       ("rust-target-lexicon" ,rust-target-lexicon-0.12)
                       ("rust-which" ,rust-which-6))))
    (home-page "https://github.com/rust-cross/cargo-zigbuild")
    (synopsis "Compile Cargo project with zig as linker")
    (description
     "This package provides Compile Cargo project with zig as linker.")
    (license license:expat)))

(concatenate-manifests
  (list
    (specifications->manifest
      (list
        "rust"
        "rust:cargo"
        "zig"
        "coreutils-minimal"
        "patchelf"
        "gcc-toolchain"
        "pkg-config"
        "eudev"
        "fontconfig"))
    ;; The GUI's MSRV is 1.80 and the daemon's 1.63. We just use the same rustc version for
    ;; both.
    ;; FIXME: be able to compile against a specified glibc (or musl) instead of having to
    ;; resort to backporting the newer rustc releases here. Also have proper Guix packages
    ;; for the two projects.
    (packages->manifest
      `(,rust-cargo-zigbuild-0.19))))
