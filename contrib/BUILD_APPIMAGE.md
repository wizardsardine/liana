## Tooling
For build an AppImage package appimagetool is needed, you can download it at 
[https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage](https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage)

Make it executable:
```shell
chmod a+x appimagetool-x86_64.AppImage
```

Store it in /usr/local/bin:
```shell
sudo mv appimagetool-x86_64.AppImage /usr/local/bin/appimagetool
```

## Build

Just run:
```shell
ARCH=x86_64 appimagetool Liana.AppDir
```

