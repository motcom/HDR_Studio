$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
Push-Location $workspace
try {
    if (!(Test-Path native/vendor/imath/CMakeLists.txt)) {
        git clone --depth 1 --branch v3.1.12 https://github.com/AcademySoftwareFoundation/Imath.git native/vendor/imath
        if ($LASTEXITCODE) { throw 'Imath download failed' }
    }
    if (!(Test-Path native/vendor/alembic/CMakeLists.txt)) {
        git clone --depth 1 --branch 1.8.9 https://github.com/alembic/alembic.git native/vendor/alembic
        if ($LASTEXITCODE) { throw 'Alembic download failed' }
    }
    cmake -S native -B native/build -A x64
    if ($LASTEXITCODE) { throw 'CMake configure failed' }
    cmake --build native/build --config Release --parallel
    if ($LASTEXITCODE) { throw 'Native build failed' }
    foreach ($profile in @('debug', 'release')) {
        New-Item -ItemType Directory -Force "target/$profile" | Out-Null
        Copy-Item -LiteralPath native/build/Release/hdr_abc.dll -Destination "target/$profile/hdr_abc.dll"
    }
    New-Item -ItemType Directory -Force native/build/licenses | Out-Null
    Copy-Item native/vendor/alembic/LICENSE.txt native/build/licenses/Alembic.txt
    Copy-Item native/vendor/imath/LICENSE.md native/build/licenses/Imath.md
} finally { Pop-Location }
