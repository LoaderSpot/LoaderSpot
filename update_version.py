#!/usr/bin/env python3
import json
import re
import sys


def update_version(data_json: str, build_type: str = None, version_file: str = "versions.json") -> bool:
    try:
        data = json.loads(data_json)
        version_key = list(data.keys())[0]
        version_data = data[version_key]

        if build_type and build_type.lower() != 'false':
            version_data = {'buildType': build_type, **version_data}

        with open(version_file, 'r', encoding='utf-8') as f:
            file_data = json.load(f)
        
        file_data[version_key] = version_data
        
        sorted_data = dict(
            sorted(
                file_data.items(),
                key=lambda x: [
                    int(part) if part.isdigit() else part
                    for part in re.split(r'(\d+)', x[0])
                ],
                reverse=True
            )
        )
        
        with open(version_file, 'w', encoding='utf-8') as f:
            json.dump(sorted_data, f, indent=2)
        
        print(f"Version {version_key} added to {version_file}")
        return True
        
    except json.JSONDecodeError as e:
        print(f"JSON parse error: {e}", file=sys.stderr)
        return False
    except FileNotFoundError:
        print(f"File {version_file} not found", file=sys.stderr)
        return False
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        return False


def main():
    if len(sys.argv) < 2:
        print("Usage: python update_version.py '<json_data>' [build_type] [version_file]", file=sys.stderr)
        sys.exit(1)
    
    data_json = sys.argv[1]
    build_type = sys.argv[2] if len(sys.argv) > 2 else None
    version_file = sys.argv[3] if len(sys.argv) > 3 else "versions.json"
    
    success = update_version(data_json, build_type, version_file)
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
