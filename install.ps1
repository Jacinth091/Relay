# Installs, upgrades or removes the Relay Windows package from GitHub Releases.
#   irm https://raw.githubusercontent.com/Jacinth091/Relay/main/install.ps1 | iex
#   & ([scriptblock]::Create((irm <url>))) -IncludePrerelease | -Version v0.1.0 | -Uninstall
# Works in Windows PowerShell 5.1 and PowerShell 7. Keep this file ASCII-only: 5.1 reads
# a BOM-less file as ANSI when run from disk.
[CmdletBinding()]
param(
    [string]$Version,
    [switch]$IncludePrerelease,
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\Relay",
    [switch]$Uninstall
)

function Install-RelayWindows {
    param(
        [string]$Version,
        [switch]$IncludePrerelease,
        [string]$InstallDir,
        [switch]$Uninstall
    )

    # Local to this function, so the caller's session preferences are left alone.
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    # Windows PowerShell 5.1 may default to older TLS, which GitHub rejects.
    if ($PSVersionTable.PSVersion.Major -lt 6) {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }

    $InstallDir = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')
    if ($InstallDir -eq [IO.Path]::GetPathRoot($InstallDir).TrimEnd('\')) {
        throw "Refusing to use a drive root as the install directory: $InstallDir"
    }

    $repo = 'https://api.github.com/repos/Jacinth091/Relay/releases'
    $apiHeaders = @{ Accept = 'application/vnd.github+json'; 'User-Agent' = 'relay-installer' }
    if ($env:GITHUB_TOKEN) { $apiHeaders.Authorization = "Bearer $env:GITHUB_TOKEN" }

    function Test-RelayRunning {
        foreach ($proc in Get-Process -Name relay -ErrorAction SilentlyContinue) {
            try { $path = $proc.Path } catch { continue }
            if ($path -and $path.StartsWith("$InstallDir\", [StringComparison]::OrdinalIgnoreCase)) {
                throw "Relay is running from $InstallDir. Close relay and re-run this command."
            }
        }
    }

    # Adds or removes $InstallDir in the user PATH and in this session's PATH. Never machine PATH.
    function Update-RelayPath {
        param([bool]$Add)
        $same = { param($entry) [Environment]::ExpandEnvironmentVariables($entry).TrimEnd('\') -ieq $InstallDir }

        $user = @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ })
        $userHasDir = @($user | Where-Object { & $same $_ }).Count -gt 0
        if ($Add -and -not $userHasDir) {
            [Environment]::SetEnvironmentVariable('Path', (@($user + $InstallDir) -join ';'), 'User')
        } elseif (-not $Add -and $userHasDir) {
            [Environment]::SetEnvironmentVariable('Path', (@($user | Where-Object { -not (& $same $_) }) -join ';'), 'User')
        }

        $session = @($env:Path -split ';' | Where-Object { $_ })
        $sessionHasDir = @($session | Where-Object { & $same $_ }).Count -gt 0
        if ($Add -and -not $sessionHasDir) {
            $env:Path = (@($session + $InstallDir) -join ';')
        } elseif (-not $Add -and $sessionHasDir) {
            $env:Path = (@($session | Where-Object { -not (& $same $_) }) -join ';')
        }
    }

    if ($Uninstall) {
        Test-RelayRunning
        if (Test-Path -LiteralPath $InstallDir) { Remove-Item -LiteralPath $InstallDir -Recurse -Force }
        Update-RelayPath -Add $false
        Write-Host "Removed Relay from $InstallDir and from your user PATH."
        Write-Host "Kept your data in $env:APPDATA\Relay (not touched)."
        return
    }

    # Release lookup: latest stable by default, -IncludePrerelease for the newest build, -Version to pin.
    if ($Version) {
        if ($Version -notmatch '^v?\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$') {
            throw "Version '$Version' is not of the form vX.Y.Z or vX.Y.Z-<suffix>."
        }
        $tag = if ($Version.StartsWith('v')) { $Version } else { "v$Version" }
    }

    try {
        if ($Version) {
            $release = Invoke-RestMethod -Uri "$repo/tags/$tag" -Headers $apiHeaders
        } elseif ($IncludePrerelease) {
            $release = Invoke-RestMethod -Uri "$repo`?per_page=30" -Headers $apiHeaders |
                Where-Object { -not $_.draft } | Select-Object -First 1
        } else {
            $release = Invoke-RestMethod -Uri "$repo/latest" -Headers $apiHeaders
        }
    } catch {
        # 404 means no matching release, or a private repository without GITHUB_TOKEN.
        if ([int]$_.Exception.Response.StatusCode -ne 404) { throw }
        $release = $null
    }
    if (-not $release) {
        throw "No Relay release found. Build from source instead (see docs/installation.md). For a private repository, set GITHUB_TOKEN."
    }

    $tag = $release.tag_name
    $zipName = "relay-$tag-windows-x86_64.zip"
    $zipAsset = $release.assets | Where-Object { $_.name -eq $zipName }
    $shaAsset = $release.assets | Where-Object { $_.name -eq "$zipName.sha256" }
    if (-not $zipAsset -or -not $shaAsset) {
        throw "Release $tag has no $zipName and its .sha256 file. Nothing was installed."
    }

    # The asset API URL works for public and private repositories alike.
    $downloadHeaders = @{ Accept = 'application/octet-stream'; 'User-Agent' = 'relay-installer' }
    if ($env:GITHUB_TOKEN) { $downloadHeaders.Authorization = "Bearer $env:GITHUB_TOKEN" }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) "relay-install-$([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Test-RelayRunning

        $zipPath = Join-Path $tmp $zipName
        $shaPath = "$zipPath.sha256"
        Invoke-WebRequest -Uri $zipAsset.url -Headers $downloadHeaders -OutFile $zipPath -UseBasicParsing
        Invoke-WebRequest -Uri $shaAsset.url -Headers $downloadHeaders -OutFile $shaPath -UseBasicParsing

        $expected = ((Get-Content -LiteralPath $shaPath -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
        if ($expected -notmatch '^[0-9a-f]{64}$') {
            throw "The checksum file for $tag is malformed. Nothing was installed."
        }
        $actual = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "SHA-256 mismatch for $zipName (expected $expected, got $actual). Nothing was installed."
        }
        Write-Host "Checksum OK for $zipName."

        $extract = Join-Path $tmp 'extract'
        Expand-Archive -LiteralPath $zipPath -DestinationPath $extract
        $files = 'relay.exe', 'LICENSE', 'README.md'
        foreach ($file in $files) {
            if (-not (Test-Path -LiteralPath (Join-Path $extract $file))) { throw "The archive is missing $file." }
        }

        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        foreach ($file in $files) {
            Copy-Item -LiteralPath (Join-Path $extract $file) -Destination $InstallDir -Force
        }
        Update-RelayPath -Add $true

        Write-Host "Installed Relay $tag to $InstallDir"
        Write-Host "Open a new terminal and type 'relay'."
    } finally {
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force }
    }
}

Install-RelayWindows -Version $Version -IncludePrerelease:$IncludePrerelease -InstallDir $InstallDir -Uninstall:$Uninstall
