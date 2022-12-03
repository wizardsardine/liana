(specifications->manifest
  (append
    (list "rust"
          "rust:cargo"
          "coreutils"
          "patchelf"
          "gcc-toolchain@10.3.0")
          ;; Additional dependencies for building the GUI
          (let ((binary_name (getenv "BINARY_NAME")))
            (if
              (string=? "liana-gui" binary_name)
              (list "pkg-config"
                    "eudev"
                    "fontconfig")
              '()))))
