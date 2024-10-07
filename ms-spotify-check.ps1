# Suppress verbose output
$VerbosePreference = 'SilentlyContinue'

function Write-Log {
    param (
        [string]$Message
    )
    Write-Host "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') - $Message"
}

function Get-LatestSpotifyVersion {
    Write-Log "Getting data from rg-adguard..."
    
    $session = New-Object Microsoft.PowerShell.Commands.WebRequestSession
    $session.UserAgent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36"

    $url = "https://store.rg-adguard.net/api/GetFiles"
    $body = "type=url&url=https://apps.microsoft.com/detail/9ncbcszsjrsb&gl=US&ring=RP&lang=en"
    $headers = @{
        "authority"          = "store.rg-adguard.net"
        "method"             = "POST"
        "path"               = "/api/GetFiles"
        "scheme"             = "https"
        "accept"             = "*/*"
        "accept-language"    = "ru,ru-RU;q=0.9,en;q=0.8"
        "dnt"                = "1"
        "origin"             = "https://store.rg-adguard.net"
        "priority"           = "u=1, i"
        "referer"            = "https://store.rg-adguard.net/"
        "sec-ch-ua"          = '"Google Chrome";v="129", "Not=A?Brand";v="8", "Chromium";v="129"'
        "sec-ch-ua-mobile"   = "?0"
        "sec-ch-ua-platform" = '"Windows"'
        "sec-fetch-dest"     = "empty"
        "sec-fetch-mode"     = "cors"
        "sec-fetch-site"     = "same-origin"
    }

    $ProgressPreference = 'SilentlyContinue'

    try {
        $response = Invoke-WebRequest -Uri $url -Method "POST" -Body $body -WebSession $session -Headers $headers -UseBasicParsing
        $html = $response.Content
    }
    catch {
        Write-Log "Error executing request: $_"
        return $null
    }

    $trContents = [regex]::Matches($html, '(?s)<tr.*?>(.*?)</tr>') | ForEach-Object { $_.Groups[1].Value }
    $results = @()
    foreach ($trContent in $trContents) {
        if ($trContent -match '<a href="(http://tlu\.dl\.delivery\.mp\.microsoft\.com[^"]+)"[^>]*>([^<]+)</a>') {
            $url = $matches[1]
            $fileName = $matches[2]
        }
        if ($trContent -match '>(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} GMT)<') {
            $dateTime = [DateTime]::ParseExact($matches[1], "yyyy-MM-dd HH:mm:ss 'GMT'", $null)
        }
        if ($trContent -match '>(\d+(?:\.\d+)?)\s*MB<') {
            $size = $matches[1] + " MB"
        }
        $results += [PSCustomObject]@{
            FileName = $fileName
            DateTime = $dateTime
            Size     = $size
            Url      = $url
        }
    }

    $filteredResults = $results | Where-Object { $_.FileName -match 'SpotifyAB\.SpotifyMusic_.+x86.+\.appx' }
    $latestFile = $filteredResults | Sort-Object -Property DateTime -Descending | Select-Object -First 1
    
    if ($latestFile) {
        Write-Log "Found: $($latestFile.FileName)"
    }
    else {
        Write-Log "No matching Spotify file found"
        Write-Log "Response:"
        Write-Log $html
    }
    return $latestFile
}

function Download-SpotifyAppx {
    param ($downloadUrl, $filePath)
    if (-not $downloadUrl) {
        Write-Log "Error: Download URL is empty or null"
        return $false
    }
    Write-Log "Downloading appx file..."
    $ProgressPreference = 'SilentlyContinue'
    try {
        Invoke-WebRequest -Uri $downloadUrl -OutFile $filePath
        return $true
    }
    catch {
        Write-Log "Error downloading file: $_"
        return $false
    }
}

function Extract-SpotifyAppx {
    param ($filePath, $extractPath)
    Write-Log "Extracting files from archive..."
    Add-Type -AssemblyName "System.IO.Compression.FileSystem"
    try {
        $zip = [System.IO.Compression.ZipFile]::OpenRead($filePath)
        $zip.Entries | Where-Object { $_.FullName -notlike '*/*' } | ForEach-Object {
            $destPath = Join-Path $extractPath $_.Name
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($_, $destPath, $true)
        }
        return $true
    }
    catch {
        Write-Log "Error extracting files: $_"
        return $false
    }
    finally {
        if ($zip -ne $null) {
            $zip.Dispose()
        }
    }
}

function Get-SpotifyExeVersion {
    param ($spotifyExePath)
    Write-Log "Getting version from file $([System.IO.Path]::GetFileName($spotifyExePath))..."
    $exeContent = Get-Content -Path $spotifyExePath -Raw
    $regex = '(?<![\w\-])(\d+)\.(\d+)\.(\d+)\.(\d+)(\.g[0-9a-f]{8})(?![\w\-])'
    if ($exeContent -match $regex) {
        Write-Log "Version received successfully: $($matches[0])"
        return $matches[0]
    }
    Write-Log "Version not found in file $([System.IO.Path]::GetFileName($spotifyExePath))"
    return $null
}

function Compare-SpotifyVersions {
    param ($version, $jsonUrl)
    Write-Log "Comparison of versions..."
    $ProgressPreference = 'SilentlyContinue'
    try {
        $jsonContent = Invoke-WebRequest -Uri $jsonUrl | ConvertFrom-Json
        foreach ($jsonVersion in $jsonContent.PSObject.Properties) {
            if ($jsonVersion.Value.fullversion -eq $version) {
                return $true
            }
        }
        return $false
    }
    catch {
        Write-Log "Error comparing versions: $_"
        return $null
    }
}

function Trigger-GitAction {
    param (
        [string]$v,
        [string]$s
    )

    $apiUrl = "https://api.github.com/repos/amd64fox/LoaderSpot/dispatches"

    $payload = @{
        event_type     = "webhook-event"
        client_payload = @{
            v = $v
            s = $s
        }
    } | ConvertTo-Json

    $headers = @{
        "Accept"        = "application/vnd.github.everest-preview+json"
        "Authorization" = "Bearer " + $env:Token
        "Content-Type"  = "application/json"
    }

    try {
        Invoke-RestMethod -Uri $apiUrl -Method Post -Headers $headers -Body $payload
        Write-Log "Successfully triggered Git action"
    }
    catch {
        Write-Log "Error triggering Git action: $_"
    }
}

function Main {
    $latestFile = Get-LatestSpotifyVersion
    if (-not $latestFile) {
        Write-Log "Failed to get latest Spotify version"
        return
    }

    $filePath = Join-Path $spotifyTempDir $latestFile.FileName
    if (-not (Download-SpotifyAppx -downloadUrl $latestFile.Url -filePath $filePath)) {
        Write-Log "Failed to download Spotify appx"
        return
    }

    if (-not (Extract-SpotifyAppx -filePath $filePath -extractPath $spotifyTempDir)) {
        Write-Log "Failed to extract Spotify appx"
        return
    }

    $spotifyExePath = Get-ChildItem -Path $spotifyTempDir -Filter "Spotify.exe" -Recurse | Select-Object -First 1
    if (-not $spotifyExePath) {
        Write-Log "Spotify.exe not found"
        return
    }

    $version = Get-SpotifyExeVersion -spotifyExePath $spotifyExePath.FullName
    if (-not $version) {
        Write-Log "Version not found in Spotify.exe"
        return
    }

    $jsonUrl = "https://raw.githubusercontent.com/amd64fox/LoaderSpot/refs/heads/main/versions.json"
    $versionExists = Compare-SpotifyVersions -version $version -jsonUrl $jsonUrl

    switch ($versionExists) {
        $false {
            Write-Log "New version found"
            Trigger-GitAction -v $version -s "[Microsoft Store](https://apps.microsoft.com/detail/9ncbcszsjrsb)"
            Write-Log "Sent for search and processing in GAS"
        }
        $null {
            Write-Log "Error comparing versions"
        }
        Default {
            Write-Log "No new version found"
        }
    }
}

$tempPath = [System.IO.Path]::GetTempPath()
$spotifyTempDir = Join-Path $tempPath "Spotify"
New-Item -Path $spotifyTempDir -ItemType Directory -Force | Out-Null

Main
