;; FIXME: Only pull the GUI dependencies when building it, not the daemon
(specifications->manifest
  (list "rust"
        "rust:cargo"
        "coreutils"
        "pkg-config" ;; For the GUI
        "eudev" ;; For the GUI
        "cmake" ;; For the GUI
        "make" ;; For the GUI
        "fontconfig" ;; For the GUI
        "gcc-toolchain@10.3.0"))
