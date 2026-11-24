param([Parameter(Mandatory)][string]$ReleaseDirectory)
$ErrorActionPreference = 'Stop'
# Prefer Microsoft's automatically serviced runtime as soon as it catches up.
$minimum = [version]'158.0.4370.33'
$installed = Get-ChildItem 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients','HKCU:\Software\Microsoft\EdgeUpdate\Clients' -ErrorAction SilentlyContinue |
    ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object name -eq 'Microsoft Edge WebView2 Runtime' |
    ForEach-Object { [version]$_.pv } | Sort-Object -Descending | Select-Object -First 1
if ($installed -ge $minimum) { return }

$base = Join-Path $env:LOCALAPPDATA 'Robotutor\runtimes'
$destination = Join-Path $base $minimum.ToString()
$executable = Join-Path $destination 'msedgewebview2.exe'
if (!(Test-Path -LiteralPath $executable)) {
    $cache = Join-Path $env:LOCALAPPDATA 'Robotutor\build-research'
    [IO.Directory]::CreateDirectory($cache) | Out-Null
    [IO.Directory]::CreateDirectory($base) | Out-Null
    $name = "Microsoft.WebView2.FixedVersionRuntime.$minimum.x64.cab"
    $archive = Join-Path $cache $name
    $sha256 = '835da387df67797f710881f553bb6934d98f24d08e370456f48d38ed4e072b62'
    if (!(Test-Path -LiteralPath $archive) -or (Get-FileHash $archive).Hash -ne $sha256) {
        $download = "$archive.$([guid]::NewGuid().ToString('N')).download"
        $url = "https://msedge.sf.dl.delivery.mp.microsoft.com/filestreamingservice/files/ecc2761e-1380-46ee-9552-995b039e4e47/$name"
        & curl.exe --fail --silent --show-error --location --max-time 900 --output $download $url
        if ($LASTEXITCODE -or (Get-FileHash $download).Hash -ne $sha256) { throw 'Microsoft WebView2 archive verification failed.' }
        Copy-Item -LiteralPath $download -Destination $archive -Force
        [IO.File]::Delete($download)
    }
    $stage = Join-Path $base ([guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($stage) | Out-Null
    & expand.exe $archive '-F:*' $stage | Out-Null
    if ($LASTEXITCODE) { throw 'Could not extract the pinned WebView2 runtime.' }
    $runtime = Get-ChildItem $stage -Recurse -Filter msedgewebview2.exe | Select-Object -First 1
    if (!$runtime) { throw 'Microsoft WebView2 executable missing from the archive.' }
    # Chromium's sandbox identities require read/execute access to runtime files.
    & icacls.exe $runtime.DirectoryName /grant '*S-1-15-2-1:(OI)(CI)(RX)' '*S-1-15-2-2:(OI)(CI)(RX)' | Out-Null
    if ($LASTEXITCODE) { throw 'Could not grant Chromium sandbox access to the runtime.' }
    try { [IO.Directory]::Move($runtime.DirectoryName,$destination) } catch [IO.IOException] {
        if (!(Test-Path -LiteralPath $executable)) { throw }
    }
}
$signature = Get-AuthenticodeSignature -LiteralPath $executable
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation' -or
    [version](Get-Item $executable).VersionInfo.ProductVersion -ne $minimum) { throw 'Microsoft WebView2 publisher or version mismatch.' }
[IO.File]::WriteAllText((Join-Path $ReleaseDirectory 'research-runtime.txt'),$destination)
