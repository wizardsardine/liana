;; FIXME: Only pull the GUI dependencies when building it, not the daemon
(specifications->manifest
  (list "rust"
        "rust:cargo"
        "coreutils"
        "pkg-config" ;; For the GUI
        "eudev" ;; For the GUI
        "fontconfig" ;; For the GUI
        "patchelf"
        "gcc-toolchain@10.3.0"))
