$ErrorActionPreference = "Stop"

$Repo = if ($env:CONTYNU_REPO) { $env:CONTYNU_REPO } else { "alentra-dev/contynu" }
$Version = if ($env:CONTYNU_VERSION) { $env:CONTYNU_VERSION } else { "latest" }
$InstallDir = if ($env:CONTYNU_INSTALL_DIR) {
    $env:CONTYNU_INSTALL_DIR
} else {
    Join-Path $HOME "AppData\Local\Programs\Contynu\bin"
}

$arch = switch ($env:PROCESSOR_ARCHITECTURE.ToLower()) {
    "amd64" { "x86_64" }
    "arm64" { "aarch64" }
    default { throw "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}

$asset = "contynu-windows-$arch.zip"
$url = if ($Version -eq "latest") {
    "https://github.com/$Repo/releases/latest/download/$asset"
} else {
    "https://github.com/$Repo/releases/download/$Version/$asset"
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("contynu-install-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    $archive = Join-Path $tmp $asset
    Write-Host "Downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $archive

    Expand-Archive -Path $archive -DestinationPath $tmp -Force
    $binary = Join-Path $tmp "contynu.exe"
    if (-not (Test-Path $binary)) {
        throw "Archive did not contain contynu.exe"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item $binary (Join-Path $InstallDir "contynu.exe") -Force

    if (-not ($env:PATH -split ";" | Where-Object { $_ -eq $InstallDir })) {
        Write-Warning "$InstallDir is not currently on PATH"
    }

    Write-Host "Installed contynu to $(Join-Path $InstallDir 'contynu.exe')"
    Write-Host "Run: contynu --help"
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
