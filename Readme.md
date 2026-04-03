# cosmic-world-clock
A world clock panel applet for the [COSMIC desktop](https://github.com/pop-os/cosmic-epoch) that displays times from your [GNOME Clocks](https://apps.gnome.org/Clocks/) world clock locations.


![](media/pict_1.png)

## Features

- Displays all your GNOME Clocks world clock locations in the panel
- Times update every minute
- Clocks are sorted by UTC offset

## Dependencies

- COSMIC desktop
- GNOME Clocks (Flatpak: `org.gnome.clocks`)

## Known Issues

- Adding or removing clocks in GNOME Clocks may take up to one minute to appear in the applet.

## Building from source
```bash
just build-release
```

## Installation
```bash
just install
```

## Credits

Based on [cosmic-applet-time](https://github.com/pop-os/cosmic-applets/tree/master/cosmic-applet-time) by System76, licensed under GPL-3.0-only.

## License

GPL-3.0-only
