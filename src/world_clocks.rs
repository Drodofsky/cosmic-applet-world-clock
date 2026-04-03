pub fn get_gnome_clocks() -> Vec<(String, String)> {
    let output = std::process::Command::new("flatpak")
        .args([
            "run",
            "--command=gsettings",
            "org.gnome.clocks",
            "get",
            "org.gnome.clocks",
            "world-clocks",
        ])
        .output();

    let Ok(output) = output else { return vec![] };
    let Ok(text) = String::from_utf8(output.stdout) else {
        return vec![];
    };

    let mut clocks = parse_gnome_clocks(&text);
    clocks.sort_by_key(|(_, tz_name)| {
        jiff::tz::TimeZone::get(tz_name)
            .map(|tz| tz.to_offset(jiff::Timestamp::now()).seconds())
            .unwrap_or(0)
    });
    clocks
}

fn parse_gnome_clocks(text: &str) -> Vec<(String, String)> {
    let finder = tzf_rs::DefaultFinder::new();

    text.split("('")
        .skip(1)
        .filter_map(|entry| {
            // Extract city name — first string before next quote
            let city = entry.split('\'').next()?.to_string();

            // Extract coordinates — find the first coordinate pair in radians
            // format: [(lat_rad, lon_rad)]
            let coords_part = entry.split("[(").nth(1)?;
            let coords = coords_part.split(')').next()?;
            let mut parts = coords.split(',');
            let lat_rad: f64 = parts.next()?.trim().parse().ok()?;
            let lon_rad: f64 = parts.next()?.trim().parse().ok()?;

            // Convert radians to degrees
            let lat = lat_rad.to_degrees();
            let lon = lon_rad.to_degrees();

            // Look up timezone from coordinates
            let tz_name = finder.get_tz_name(lon, lat).to_string();

            Some((city, tz_name))
        })
        .collect()
}
