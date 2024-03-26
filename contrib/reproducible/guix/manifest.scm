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
            ;; Additional dependencies for building the GUI.
            (if
              (string=? is_gui "1")
              (list "pkg-config"
                    "eudev"
                    "fontconfig")
              '())))
      ;; The GUI's MSRV is 1.65 and the daemon's 1.63.
      ;; FIXME: be able to compile against a specified glibc (or musl) instead of having to
      ;; resort to not bumping the time machine and use at-the-time unpublished rustc.
      (packages->manifest
        (if
          (string=? is_gui "1")
          (list
            (@@ (gnu packages rust) rust-1.65))
          (list
            (@@ (gnu packages rust) rust-1.63)))))))
