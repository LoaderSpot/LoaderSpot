import re
import json
import aiohttp
import asyncio
from tqdm import tqdm
from typing import Optional, Tuple, List, Dict
from dataclasses import dataclass
from enum import Enum


class Platform(Enum):
    WIN_X86 = "Win-x86"
    WIN_X64 = "Win-x64"
    WIN_ARM64 = "Win-arm64"
    MACOS_INTEL = "macOS-intel"
    MACOS_ARM64 = "macOS-arm64"


@dataclass
class SpotifyVersion:
    version: str
    start_number: int
    end_number: int


def extract_base_version(version: str) -> str:
    parts = version.split('.')
    if len(parts) >= 3:
        return f"{parts[0]}.{parts[1]}.{parts[2]}"
    return version


def should_use_win_x86(version: str) -> bool:
    base_version = extract_base_version(version)
    try:
        # Split by dot and compare parts
        parts = base_version.split('.')
        version_tuple = (int(parts[0]), int(parts[1]), int(parts[2]))
        
        # Compare with version 1.2.53 using tuple comparison
        return version_tuple <= (1, 2, 53)
    except (ValueError, IndexError):
        # If there's any error in parsing, default to include
        return True


class UrlGenerator:
    BASE_URL = "https://upgrade.scdn.co/upgrade/client/"
    PLATFORM_PATHS = {
        Platform.WIN_X86: "win32-x86/spotify_installer-{version}-{number}.exe",
        Platform.WIN_X64: "win32-x86_64/spotify_installer-{version}-{number}.exe",
        Platform.WIN_ARM64: "win32-arm64/spotify_installer-{version}-{number}.exe",
        Platform.MACOS_INTEL: "osx-x86_64/spotify-autoupdate-{version}-{number}.tbz",
        Platform.MACOS_ARM64: "osx-arm64/spotify-autoupdate-{version}-{number}.tbz",
    }

    @staticmethod
    def generate_url(platform: Platform, version: str, number: int) -> str:
        return f"{UrlGenerator.BASE_URL}{UrlGenerator.PLATFORM_PATHS[platform].format(version=version, number=number)}"


async def check_url(
    session: aiohttp.ClientSession, url: str, platform: Platform
) -> Optional[Tuple[str, Platform]]:
    try:
        async with session.head(url) as response:
            if response.status == 200:
                return url, platform
    except aiohttp.ClientError:
        pass
    return None


async def fetch_versions_json(session: aiohttp.ClientSession) -> dict:
    async with session.get(
        "https://raw.githubusercontent.com/amd64fox/LoaderSpot/refs/heads/main/versions.json"
    ) as response:
        if response.status == 200:
            try:
                return json.loads(await response.text())
            except json.JSONDecodeError:
                return {}
        return {}


async def submit_to_google_form(session: aiohttp.ClientSession, version: str, comment: str = "from LoaderSpot") -> None:
    form_url = "https://docs.google.com/forms/u/0/d/e/1FAIpQLSdqIxSjqt2PcjBlQzhvwqc4QckfWuq5qqWsrdpoTidQHsPGpw/formResponse"
    data = {"entry.1104502920": version, "entry.1319854718": comment}

    try:
        await session.post(form_url, data=data)
    except Exception:
        pass


async def check_version_and_submit(session: aiohttp.ClientSession, version: str) -> None:
    try:
        versions_json = await fetch_versions_json(session)
        version_exists = any(
            ver_data.get("fullversion") == version
            for ver_data in versions_json.values()
        )

        if not version_exists:
            await submit_to_google_form(session, version)

    except Exception:
        pass


def validate_version(version: str) -> bool:
    return bool(re.match(r"^\d+\.\d+\.\d+\.\d+\.g[0-9a-f]{8}$", version))


def validate_number(number: str) -> bool:
    return number.isdigit()


def validate_range(start: int, end: int) -> bool:
    return end >= start and (end - start) <= 20000


def get_valid_input(prompt: str, validator: callable, error_message: str) -> str:
    while True:
        value = input(prompt)
        if validator(value):
            return value
        print(error_message)


def get_version_input() -> str:
    while True:
        version = input("Spotify version, for example 1.1.68.632.g2b11de83: ")
        if validate_version(version):
            return version
        print("Invalid version format")


def calculate_total_urls(
    start_number: int, end_number: int, selected_platforms: List[Platform]
) -> int:
    return (end_number - start_number + 1) * len(selected_platforms)


async def search_installers(
    session: aiohttp.ClientSession, version_info: SpotifyVersion, selected_platforms: List[Platform]
) -> Dict[Platform, List[str]]:
    tasks = []
    total_urls = calculate_total_urls(
        version_info.start_number, version_info.end_number, selected_platforms
    )

    for platform in selected_platforms:
        for number in range(version_info.start_number, version_info.end_number + 1):
            url = UrlGenerator.generate_url(platform, version_info.version, number)
            tasks.append(check_url(session, url, platform))

    results: Dict[Platform, List[str]] = {platform: [] for platform in Platform}

    avg_speed = [0, 0] 
    
    custom_bar_format = "{desc}: {percentage:3.0f}%|{bar}| {n_fmt}/{total_fmt} [{elapsed}<{remaining}"
    
    with tqdm(total=total_urls, desc="Checking URLs", 
              bar_format=custom_bar_format + ", ? urls/sec]") as pbar:
        
        def update_speed(rate):
            nonlocal custom_bar_format
            speed_str = f", {int(rate)} urls/sec]"
            pbar.bar_format = custom_bar_format + speed_str
        
        for task in asyncio.as_completed(tasks):
            if result := await task:
                url, platform = result
                results[platform].append(url)
            
            pbar.update(1)
            
            rate = pbar.format_dict.get('rate', 0)
            if rate > 0:
                avg_speed[0] += rate
                avg_speed[1] += 1
                update_speed(rate)
        
        if avg_speed[1] > 0:
            avg_rate = avg_speed[0] / avg_speed[1]
            final_speed_str = f", {int(avg_rate)} urls/sec (avg)]"
            pbar.bar_format = custom_bar_format + final_speed_str
            pbar.refresh() 

    return results


def display_results(results: Dict[Platform, List[str]]) -> None:
    found_any = False
    for platform in Platform:
        if urls := results[platform]:
            found_any = True
            print(f"\n{platform.value}:")
            for url in urls:
                print(url)

    if not found_any:
        print("\nNothing found, consider increasing the search range")


def get_platform_choices(version_spoti: str = "") -> List[Platform]:
    print("\nSelect the link type for the search:")
    
    # Filter out WIN_X86 platform for versions > 1.2.53
    available_platforms = list(Platform)
    if version_spoti and not should_use_win_x86(version_spoti):
        available_platforms = [p for p in Platform if p != Platform.WIN_X86]
    
    for i, platform in enumerate(available_platforms, 1):
        print(f"[{i}] {platform.value}")
    print(f"[{len(available_platforms) + 1}] All platforms")

    while True:
        choices = input("Enter the number(s): ").strip().split(",")

        if not all(choice.strip().isdigit() and 1 <= int(choice.strip()) <= len(available_platforms) + 1 for choice in choices):
            print(f"Invalid input. Please enter numbers between 1 and {len(available_platforms) + 1}")
            continue

        if str(len(available_platforms) + 1) in choices:
            return available_platforms

        selected = []
        for choice in choices:
            platform_index = int(choice.strip()) - 1
            if 0 <= platform_index < len(available_platforms):
                selected.append(available_platforms[platform_index])

        if selected:
            return selected

        print("Please select at least one valid platform")


def get_max_connections() -> int:
    while True:
        max_connections = input("Maximum number of concurrent connections (default 100): ").strip()
        if not max_connections:
            return 100
        if max_connections.isdigit() and int(max_connections) > 0:
            return int(max_connections)
        print("Please enter a valid positive number")


async def main(version_spoti: str = "") -> None:
    # Increase connection limit
    max_connections = get_max_connections()
    connector = aiohttp.TCPConnector(limit=max_connections)
    async with aiohttp.ClientSession(connector=connector) as session:
        if not version_spoti:
            version_spoti = get_version_input()

        version_check_task = asyncio.create_task(check_version_and_submit(session, version_spoti))

        start_number = int(
            get_valid_input(
                "Start search from: ",
                lambda x: validate_number(x),
                "Please enter a valid number",
            )
        )
        end_number = int(
            get_valid_input(
                "End search at: ",
                lambda x: validate_number(x) and validate_range(start_number, int(x)),
                f"Please enter a valid number that is at least {start_number} and no more than {start_number + 20000}",
            )
        )

        version_info = SpotifyVersion(version_spoti, start_number, end_number)
        selected_platforms = get_platform_choices(version_spoti)

        print("\nSearching...\n")
        results = await search_installers(session, version_info, selected_platforms)
        display_results(results)

        try:
            await asyncio.wait_for(version_check_task, timeout=1.0)
        except asyncio.TimeoutError:
            pass

        print("\nChoose an option:")
        print("[1] Perform the search with a new version")
        print("[2] Perform the search again with the same version")
        print("[3] Exit")

        choice = input("Enter the number: ")

        if choice == "1":
            print("\n")
            await main() 
        elif choice == "2":
            print("\n")
            print(f"Search version: {version_spoti}")
            await main(version_spoti) 
        elif choice == "3":
            return
        else:
            print("Invalid choice. Exiting the program.")


if __name__ == "__main__":
    asyncio.run(main())
