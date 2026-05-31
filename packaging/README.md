# Packaging

Files for OS-level integration (file associations, launcher entries).

## Linux

| File                    | Installs to                              | Purpose                                              |
| ----------------------- | ---------------------------------------- | ---------------------------------------------------- |
| `datafusion-ui.desktop` | `/usr/share/applications/`               | Launcher entry; declares the `.parquet` MIME handler |
| `datafusion-ui.xml`     | `/usr/share/mime/packages/`              | Maps `*.parquet` → `application/vnd.apache.parquet`  |

System-wide install:

```sh
install -Dm644 packaging/datafusion-ui.desktop /usr/share/applications/datafusion-ui.desktop
install -Dm644 packaging/datafusion-ui.xml     /usr/share/mime/packages/datafusion-ui.xml
update-mime-database /usr/share/mime
update-desktop-database /usr/share/applications
```

Per-user install (no root):

```sh
install -Dm644 packaging/datafusion-ui.desktop ~/.local/share/applications/datafusion-ui.desktop
install -Dm644 packaging/datafusion-ui.xml     ~/.local/share/mime/packages/datafusion-ui.xml
update-mime-database ~/.local/share/mime
update-desktop-database ~/.local/share/applications
```

The `.desktop` `StartupWMClass=datafusion-ui` must match the window `application_id`
set in `src/main.rs` so KDE/GNOME clear the startup "busy" cursor when the window maps.

