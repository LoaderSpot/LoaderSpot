param (
    [string]$versions,
    [string]$source,
    [string]$googleAppsUrl
)

function Find-BuildInfo {
    [CmdletBinding()]
    param (
        [Parameter(Mandatory=$true, ValueFromPipeline=$true, Position=0)]
        [string]$Path
    )

    process {
        try {
            $fileContent = Get-Content -Path $Path -ErrorAction Stop
            $found = $false

            $regex = '(Master|Release|PR|Local) Build.+(?:cef_)?(\d+\.\d+\.\d+\+g[0-9a-f]+\+chromium-\d+\.\d+\.\d+\.\d+)'

            foreach ($line in $fileContent) {
                $match = [regex]::Match($line, $regex)
                if ($match.Success) {
                    $buildType = $match.Groups[1].Value
                    Write-Host "build type: $buildType"
                    return $buildType
                }
            }

            if (-not $found) {
                Write-Host "Билд не найден"
                return $false
            }
        }
        catch {
            Write-Error "Ошибка при чтении файла: $_"
            return $false
        }
    }
}

function Get-BuildTypeFromUrl {
    [CmdletBinding()]
    param(
        [string]$Url
    )
    
    $tempPath = Join-Path ([System.IO.Path]::GetTempPath()) "spotify"
    $destinationPath = Join-Path $tempPath "unpacked"
    $exePath = Join-Path $tempPath "SpotifySetup.exe"

    if (-not (Test-Path -Path $tempPath)) {
        New-Item -ItemType Directory -Path $tempPath | Out-Null
    }
    if (-not (Test-Path -Path $destinationPath)) {
        New-Item -ItemType Directory -Path $destinationPath | Out-Null
    }

    try {
        Write-Host "Скачивание файла из $Url..."
        Invoke-WebRequest -Uri $Url -OutFile $exePath
        Write-Host "Файл сохранен в $exePath"

        Write-Host "Распаковка файла в $destinationPath"
        Start-Process -Wait -FilePath $exePath -ArgumentList "/extract `"$destinationPath`""

        $dllPath = Join-Path $destinationPath "Spotify.dll"
        $exePathForAnalysis = Join-Path $destinationPath "Spotify.exe"

        if (Test-Path $dllPath) {
            return Find-BuildInfo -Path $dllPath
        } elseif (Test-Path $exePathForAnalysis) {
            return Find-BuildInfo -Path $exePathForAnalysis
        }
        else {
            Write-Error "Файлы Spotify.dll и Spotify.exe не найдены в $destinationPath"
            return $false
        }
    }
    catch {
        Write-Error "Произошла ошибка при скачивании или распаковке: $_"
        return $false
    }
}

if ([string]::IsNullOrEmpty($versions) -or [string]::IsNullOrEmpty($source) -or [string]::IsNullOrEmpty($googleAppsUrl)) {
    Write-Error "Один или несколько обязательных параметров (versions, source, googleAppsUrl) не предоставлены."
    exit 1
}

$versionsObj = $versions | ConvertFrom-Json
$win64Url = $versionsObj.WIN64
$buildType = $false

if ($win64Url) {
    $buildType = Get-BuildTypeFromUrl -Url $win64Url
}

if ($buildType -eq $false) {
    Write-Host "Тип сборки не определен, отправка в GAS отменена."
    exit 0
}

$versionsObj | Add-Member -NotePropertyName "buildType" -NotePropertyValue $buildType -Force
$versionsObj | Add-Member -NotePropertyName "source" -NotePropertyValue $source -Force

$finalJson = $versionsObj | ConvertTo-Json -Compress

Write-Host "Отправка данных на GAS..."

try {
    $response = Invoke-WebRequest -Uri $googleAppsUrl `
        -Method POST `
        -ContentType "application/json" `
        -Body $finalJson `
        -UseBasicParsing -ErrorAction Stop
    
    if ($response.StatusCode -eq 200) {
        Write-Host "Ответ от GAS: $($response.Content)"
    } else {
        Write-Error "Ошибка при отправке в GAS. Статус: $($response.StatusCode). Ответ: $($response.Content)"
    }
} catch {
    Write-Error "Критическая ошибка при отправке в GAS: $_"
}