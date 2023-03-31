(let ((is_gui (getenv "IS_GUI")))
  (concatenate-manifests
    (list
      (specifications->manifest
        (append
          (list
            "rust:cargo"
            "coreutils-minimal"
            "patchelf"
            "gcc-toolchain@10.3.0")
            ;; Additional dependencies for building the GUI, and the regular rustc for building
            ;; the daemon and CLI.
            (if
              (string=? is_gui "1")
              (list "pkg-config"
                    "eudev"
                    "fontconfig")
              (list "rust"))))
      ;; The GUI MSRV is 1.65. We could patch the requirements for new compiler features but
      ;; Iced 0.7 started relying on a lifetime that was treated as a bug until rustc 1.64. I
      ;; couldn't find how to fix it therefore we are relying on a Rust toolchain unpublished
      ;; by Guix to build the GUI.
      (packages->manifest
        (if
          (string=? is_gui "1")
          (list
            (@@ (gnu packages rust) rust-1.65))
          '())))))
