pub fn create_directory(datadir_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    return {
        use std::fs::DirBuilder;
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = DirBuilder::new();
        builder.mode(0o700).recursive(true).create(datadir_path)?;
        Ok(())
    };

    // TODO: permissions on Windows..
    #[cfg(not(unix))]
    return {
        std::fs::create_dir_all(datadir_path)?;
        Ok(())
    };
}
