use std::path::PathBuf;

#[cfg(windows)]
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::process::{Command, Stdio};

#[cfg(windows)]
const RELEASE_URL: &str = "https://github.com/cloudflare/cloudflared/releases/download/2026.8.2/cloudflared-windows-amd64.exe";
#[cfg(windows)]
const RELEASE_SHA256: &str = "c29eee2b121f5436a642eed69fd9767da7e7b8c510fa50aaa130337f931357b5";

pub(crate) fn is_verified(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        std::fs::read(path)
            .is_ok_and(|bytes| format!("{:x}", Sha256::digest(bytes)) == RELEASE_SHA256)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

pub(crate) fn download() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| "LOCALAPPDATA is not set; cannot install cloudflared.".to_owned())?;
        let directory = base.join("Chessrena");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("Could not create the Chessrena app-data folder: {error}"))?;
        let destination = directory.join("cloudflared.exe");
        let temporary = directory.join("cloudflared.download.tmp");
        let script = r#"$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; $temp = $env:CHESSRENA_CLOUDFLARED_TEMP; try { Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue; Invoke-WebRequest -UseBasicParsing -TimeoutSec 20 -Uri $env:CHESSRENA_CLOUDFLARED_URL -OutFile $temp; $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $temp).Hash.ToLowerInvariant(); if ($actual -ne $env:CHESSRENA_CLOUDFLARED_SHA256) { throw 'SHA-256 verification failed'; }; Move-Item -LiteralPath $temp -Destination $env:CHESSRENA_CLOUDFLARED_DEST -Force; exit 0 } catch { Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue; exit 1 }"#;
        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("CHESSRENA_CLOUDFLARED_URL", RELEASE_URL)
            .env("CHESSRENA_CLOUDFLARED_SHA256", RELEASE_SHA256)
            .env("CHESSRENA_CLOUDFLARED_TEMP", &temporary)
            .env("CHESSRENA_CLOUDFLARED_DEST", &destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|error| format!("Could not start the Cloudflare download: {error}"))?;
        if !output.status.success() {
            return Err(
                "Download or SHA-256 verification failed. No executable was installed.".to_owned(),
            );
        }
        Ok(destination)
    }
    #[cfg(not(windows))]
    {
        Err("Automatic cloudflared download is supported only on Windows; install it from Cloudflare's official downloads page.".to_owned())
    }
}
