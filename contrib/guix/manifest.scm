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
              (string=? binary_name "liana-gui")
              (list "pkg-config"
                    "eudev"
                    "fontconfig")
              '()))))
