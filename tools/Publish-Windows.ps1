[CmdletBinding()]
param(
    [string]$BuildDirectory = (Join-Path $env:LOCALAPPDATA 'Robotutor\build'),
    [hashtable]$BuildCaches = @{},
    [string]$CertificateThumbprint = $env:ROBOTUTOR_SIGNING_THUMBPRINT,
    [ValidateSet('Auto','Native','Container')][string]$Compiler = 'Auto',
    [switch]$TestResearch
)
$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$packages = [ordered]@{
    'robotutor-desktop' = 'desktop/native/Cargo.toml'
    'tutor' = 'lab/Cargo.toml'
    'robotutor-lms' = 'lms/Cargo.toml'
    'robotutor-courses' = 'desktop/course-window/Cargo.toml'
    'robotutor-research' = 'desktop/course-window/Cargo.toml'
}
$outputs = @{}
if ($Compiler -eq 'Auto') {
    $policy = Get-CimInstance -Namespace root\Microsoft\Windows\DeviceGuard -ClassName Win32_DeviceGuard -ErrorAction SilentlyContinue
    $Compiler = if ($policy.UsermodeCodeIntegrityPolicyEnforcementStatus -eq 2) { 'Container' } else { 'Native' }
}
if ($Compiler -eq 'Container') {
    # Only source and the fresh output directory are mounted. The Windows key
    # stays in CurrentUser\My. Cargo build scripts run in the Linux container.
    $engine = Get-Command podman,docker -ErrorAction SilentlyContinue | Where-Object {
        try { & $_.Source info *> $null; $LASTEXITCODE -eq 0 } catch { $false }
    } | Select-Object -First 1
    if (!$engine) {
        $podman = Get-Command podman -ErrorAction SilentlyContinue
        if ($podman) {
            & $podman.Source machine start
            if ($LASTEXITCODE -eq 0) { $engine = $podman }
        }
    }
    if (!$engine) { throw 'Configure a Podman machine or start Docker Desktop for the isolated Windows build.' }
    $image = 'docker.io/messense/cargo-xwin@sha256:19a3d2f62c5ae8af5424498f219cad17ac99cb12900305d2485018ac4c61cad5'
    $out = Join-Path $BuildDirectory ('cross-output\' + [Guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($out) | Out-Null
    $argsList = @('run','--rm','--platform','linux/amd64','-v',($project + ':/src:ro'),'-v',($out + ':/output'),
        '-v','robotutor-msvc-build:/build','-v','robotutor-msvc-registry:/usr/local/cargo/registry',
        '-v','robotutor-msvc-rustup:/usr/local/rustup',
        '-v','robotutor-msvc-sdk:/sdk','-e','CARGO_TARGET_DIR=/build','-e','XWIN_CACHE_DIR=/sdk',
        '-e','XWIN_ARCH=x86_64','-e',("ROBOTUTOR_TEST_RESEARCH=" + [int]$TestResearch.IsPresent),
        '-w','/src',$image,'sh','/src/tools/windows/build-msvc.sh')
    foreach ($name in $packages.Keys) {
        $argsList += @($name, $packages[$name])
        $outputs[$name] = Join-Path $out "$name.exe"
    }
    & $engine.Source @argsList
    if ($LASTEXITCODE) { throw 'The isolated build failed. The previous release is still selected.' }
} else {
    foreach ($name in $packages.Keys) {
        # Cargo's intermediate cache can be shared with earlier builds. Executables are
        # emitted outside both that cache and the directory currently used by the app.
        $cache = if ($BuildCaches.ContainsKey($name)) { $BuildCaches[$name] } else { $BuildDirectory }
        $env:CARGO_BUILD_BUILD_DIR = [IO.Path]::GetFullPath($cache)
        $env:CARGO_TARGET_DIR = Join-Path $BuildDirectory 'unsigned'
        $outputs[$name] = Join-Path $env:CARGO_TARGET_DIR "debug\$name.exe"
        & cargo build --locked --manifest-path (Join-Path $project $packages[$name]) --bin $name
        if ($LASTEXITCODE) { throw "Cargo could not build $name. See the compiler diagnostic above." }
    }
    if ($TestResearch) {
        & cargo test --locked --manifest-path (Join-Path $project $packages['robotutor-research']) --bin robotutor-research
        if ($LASTEXITCODE) { throw 'Research browser verification failed.' }
    }
}

$certs = @(Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert | Where-Object {
    $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date) -and $_.NotBefore -le (Get-Date)
})
if ($CertificateThumbprint) {
    $cert = $certs | Where-Object Thumbprint -eq $CertificateThumbprint | Select-Object -First 1
    if (!$cert) { throw 'Configured signing identity is unavailable in CurrentUser\My.' }
} else {
    $cert = $certs | Where-Object Subject -eq 'CN=Robotutor Local Development' |
        Sort-Object NotAfter -Descending | Select-Object -First 1
    if (!$cert) {
        $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN=Robotutor Local Development' `
            -CertStoreLocation Cert:\CurrentUser\My -KeyAlgorithm RSA -KeyLength 3072 `
            -KeyExportPolicy NonExportable -HashAlgorithm SHA256 -NotAfter (Get-Date).AddYears(2)
    }
}

# Sign a new copy. A running lesson and an existing LMS process may still have their images mapped.
$releases = Join-Path $env:LOCALAPPDATA 'Robotutor\releases'
[IO.Directory]::CreateDirectory($releases) | Out-Null
$release = Join-Path $releases ([Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($release) | Out-Null
[IO.File]::WriteAllText((Join-Path $release "project-root.txt"),$project)
foreach ($name in $packages.Keys) {
    $dest = Join-Path $release "$name.exe"
    Copy-Item -LiteralPath $outputs[$name] -Destination $dest
    $signed = Set-AuthenticodeSignature -FilePath $dest -Certificate $cert -HashAlgorithm SHA256
    $checked = Get-AuthenticodeSignature -LiteralPath $dest
    if (!$checked.SignerCertificate -or $checked.SignerCertificate.Thumbprint -ne $cert.Thumbprint) {
        throw "Signing failed for $name. $($signed.StatusMessage)"
    }
    # UntrustedRoot is expected for a development certificate in the personal store.
    if ($checked.Status -ne 'Valid') {
        $chain = New-Object Security.Cryptography.X509Certificates.X509Chain
        $chain.ChainPolicy.RevocationMode = 'NoCheck'
        [void]$chain.Build($cert)
        $failures = @($chain.ChainStatus | ForEach-Object { $_.Status.ToString() })
        $chain.Dispose()
        if ($checked.Status -notin 'NotTrusted','UnknownError' -or $failures.Count -ne 1 -or $failures[0] -ne 'UntrustedRoot' -or
            $checked.StatusMessage -notmatch 'root certificate which is not trusted|untrusted root|not trusted by the trust provider') {
            throw "Signature verification failed for $name. $($checked.StatusMessage)"
        }
    }
    Write-Output "$name signed by $($cert.Thumbprint); Windows trust status $($checked.Status)"
}
& (Join-Path $PSScriptRoot "windows/Stage-ResearchRuntime.ps1") -ReleaseDirectory $release
# Signature presence and permission to execute are separate Windows checks.
# Leave the previous installation selected if this PC denies any new component.
foreach ($name in $packages.Keys) {
    try {
        $probe = Start-Process -FilePath (Join-Path $release "$name.exe") -ArgumentList '--version' -WindowStyle Hidden -PassThru
        if (!$probe.WaitForExit(15000) -or $probe.ExitCode -ne 0) { throw 'Version probe failed.' }
    } catch {
        throw "The signed $name could not run. The staged package is at $release. $($_.Exception.Message)"
    }
}
# Update the launcher pointer only after every project binary has its signature.
$pointer = Join-Path (Split-Path $releases) 'desktop-release.txt'
$temp = "$pointer.$([Guid]::NewGuid().ToString('N')).tmp"
[IO.File]::WriteAllText($temp, $release, [Text.UTF8Encoding]::new($false))
if ([IO.File]::Exists($pointer)) { [IO.File]::Replace($temp,$pointer,"$temp.previous"); [IO.File]::Delete("$temp.previous") } else { [IO.File]::Move($temp,$pointer) }
Write-Output "Installed $release"
