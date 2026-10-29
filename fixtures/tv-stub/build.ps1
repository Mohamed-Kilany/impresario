# Build the shared Impresario TV stub for webOS and Tizen.
#
# The HTML, icon and media are shared. Each package gets only its own manifest:
# webOS uses appinfo.json, Tizen uses config.xml. Tizen TV package IDs are
# limited to 10 characters, so the Tizen manifest uses package "impresario"
# with app id "impresario.stub".
param(
    [string]$Impresario = ""
)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
if (-not $Impresario) {
    $Impresario = Join-Path (Resolve-Path (Join-Path $root '..\..')) 'target\release\impresario.exe'
}
$build = Join-Path $root 'build'
$shared = @('icon.png', 'impresario-hook.js', 'index.html', 'media_sample.mp4', 'sample_image.png')

foreach ($platform in @('webos', 'tizen')) {
    $stage = Join-Path $build "stage-$platform"
    if (Test-Path $stage) {
        Remove-Item $stage -Recurse -Force
    }
    New-Item -ItemType Directory -Path $stage | Out-Null
    foreach ($file in $shared) {
        Copy-Item (Join-Path $root $file) $stage
    }
    if ($platform -eq 'webos') {
        Copy-Item (Join-Path $root 'appinfo.json') $stage
        $output = Join-Path $build 'stub.ipk'
    } else {
        Copy-Item (Join-Path $root 'config.xml') $stage
        $output = Join-Path $build 'stub.wgt'
    }
    & $Impresario package $platform $stage -o $output --no-tui --plain --no-color
    if ($LASTEXITCODE -ne 0) {
        throw "packaging $platform failed with exit code $LASTEXITCODE"
    }
}
