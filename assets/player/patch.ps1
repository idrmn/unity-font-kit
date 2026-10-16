param([ValidateSet("install", "restore")] [string] $Action)
$ErrorActionPreference = "Stop"
$patch = $PSScriptRoot
$folder = (Get-Content -LiteralPath "$patch\folder.txt" -Encoding UTF8 -TotalCount 1).Trim()
$remembered = "$patch\game.txt"
$backup = "$patch\backup"

function Get-Sha256($path) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return "" }
    (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLower()
}

function Find-Data {
    $saved = if (Test-Path -LiteralPath $remembered) { "$(Get-Content -LiteralPath $remembered -Encoding UTF8 -TotalCount 1)".Trim() }
    # An install stopped while writing game.txt leaves it empty, and a moved game leaves a stale path.
    if ($saved -and (Test-Path -LiteralPath $saved -PathType Container)) { return $saved }
    $next = Join-Path (Split-Path $patch) $folder
    if (Test-Path -LiteralPath $next -PathType Container) { return $next }
    Write-Host "Cannot find the game folder next to this file."
    Write-Host "Drag the game folder, the one that has $folder inside, into this window, then press Enter."
    $answer = (Read-Host "Game folder").Trim().Trim('"')
    foreach ($candidate in @((Join-Path $answer $folder), $answer)) {
        if ((Split-Path $candidate -Leaf) -eq $folder -and (Test-Path -LiteralPath $candidate -PathType Container)) {
            return $candidate
        }
    }
    throw "There is no $folder in $answer. Choose the folder of the game this patch is for."
}

function Read-Files($data) {
    Get-Content -LiteralPath "$patch\files.tsv" -Encoding UTF8 | Where-Object { $_ } | ForEach-Object {
        $original, $patched, $delta, $file = $_ -split "`t", 4
        $game = Join-Path $data $file
        # Short names keep backups under the 260-character path limit.
        $name = [System.IO.Path]::GetFileNameWithoutExtension($delta)
        [pscustomobject]@{
            Path = $file; Original = $original; Patched = $patched; Game = $game
            Delta = Join-Path $patch $delta; Backup = Join-Path $backup $name; Partial = "$game.ufk-partial"
            Current = Get-Sha256 $game
        }
    }
}

function Write-Checked($f, $hash, [scriptblock] $write) {
    & $write $f.Partial
    if ((Get-Sha256 $f.Partial) -ne $hash) {
        Remove-Item -LiteralPath $f.Partial -ErrorAction SilentlyContinue
        throw "Could not write $($f.Game)."
    }
    # Next to the game file and swapped in one step, so a stop halfway never leaves the game without it.
    # PowerShell turns $null into "" for a string parameter, which Replace rejects.
    [System.IO.File]::Replace($f.Partial, $f.Game, [NullString]::Value)
}

function Install-Fonts {
    $data = Find-Data
    $files = @(Read-Files $data)
    $wrong = @($files | Where-Object { $_.Current -ne $_.Original -and $_.Current -ne $_.Patched })
    if ($wrong) {
        Write-Host "These game files are not the version this patch is for:"
        $wrong | ForEach-Object { Write-Host "  $($_.Path)" }
        throw "Nothing was changed. Ask the author of the patch which game version it is for."
    }
    # Written first, so restore works even if the install stops halfway.
    Set-Content -LiteralPath $remembered -Value $data -Encoding UTF8
    New-Item -ItemType Directory -Force -Path $backup | Out-Null
    foreach ($f in $files | Where-Object { $_.Current -eq $_.Original }) {
        Copy-Item -LiteralPath $f.Game -Destination $f.Backup -Force
        Write-Checked $f $f.Patched {
            param($out)
            & "$patch\xdelta3decode.exe" -d -f -s $f.Game $f.Delta $out
        }
        Write-Host "Patched $($f.Path)"
    }
    $unsaved = @($files | Where-Object { $_.Current -eq $_.Patched -and (Get-Sha256 $_.Backup) -ne $_.Original })
    if ($unsaved) {
        Write-Host "These files were already patched, and ufk-patch has no copy of their originals:"
        $unsaved | ForEach-Object { Write-Host "  $($_.Path)" }
        Write-Host "Restore cannot put them back. To undo, let Steam or your game launcher check the game files."
    }
    Write-Host "Done. Start the game and check the text."
}

function Restore-Files {
    if (-not (Test-Path -LiteralPath $remembered)) { throw "The fonts are not installed, nothing to restore." }
    $data = Find-Data
    $files = @(Read-Files $data)
    $changed = @($files | Where-Object {
        $_.Current -ne $_.Original -and -not ($_.Current -eq $_.Patched -and (Get-Sha256 $_.Backup) -eq $_.Original)
    })
    if ($changed) {
        Write-Host "These game files were changed after the install, for example by a game update:"
        $changed | ForEach-Object { Write-Host "  $($_.Path)" }
        throw "Nothing was changed. Let Steam or your game launcher check the game files instead."
    }
    foreach ($f in $files | Where-Object { $_.Current -eq $_.Patched }) {
        Write-Checked $f $f.Original { param($out) Copy-Item -LiteralPath $f.Backup -Destination $out -Force }
        Write-Host "Restored $($f.Path)"
    }
    Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $remembered
    Write-Host "Done. The game has its original files again."
}

try {
    if ($Action -eq "install") { Install-Fonts } else { Restore-Files }
} catch [System.UnauthorizedAccessException] {
    Write-Host "Windows does not let this script change the game files. Close the game and try again."
    Write-Host "If the game is in Program Files, right-click the .bat file and choose Run as administrator."
    exit 1
} catch {
    Write-Host $_.Exception.Message
    exit 1
}
