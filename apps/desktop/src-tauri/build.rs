fn main() {
    let mut attrs = tauri_build::Attributes::new();

    // Windows: leggere \\.\PhysicalDriveN richiede i permessi di amministratore.
    // Nelle build di release l'eseguibile li chiede all'avvio (finestra UAC).
    // "highestAvailable": un utente senza diritti di amministratore può comunque
    // avviare l'app e aprire le immagini disco. In sviluppo (`tauri dev`) il
    // manifest resta quello predefinito, così non serve un terminale elevato.
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        let windows = tauri_build::WindowsAttributes::new()
            .app_manifest(include_str!("windows-app-manifest.xml"));
        attrs = attrs.windows_attributes(windows);
    }

    tauri_build::try_build(attrs).expect("errore nello script di build di Tauri");
}
