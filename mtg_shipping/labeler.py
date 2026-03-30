#!/usr/bin/env python3
import sys, datetime, argparse, json, os, subprocess, shutil
import xml.etree.ElementTree as ET

CACHE_FILE = os.path.expanduser("~/.address_cache.json")
USER_ID = 'YOUR_USPS_USER_ID'

def escape_ps(text):
    return text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")

def load_cache():
    if os.path.exists(CACHE_FILE):
        with open(CACHE_FILE, 'r') as f:
            try: return json.load(f)
            except: return {}
    return {}

def save_cache(cache):
    with open(CACHE_FILE, 'w') as f:
        json.dump(cache, f, indent=4)

def fetch_usps(addr, csz):
    import requests
    url = "http://production.shippingapis.com/ShippingAPI.dll"
    xml = f'<AddressValidateRequest USERID="{USER_ID}"><Address><Address1></Address1><Address2>{addr}</Address2><City></City><State></State><Zip5>{csz.split()[-1][:5]}</Zip5><Zip4></Zip4></Address></AddressValidateRequest>'
    try:
        r = requests.get(url, params={'API': 'Verify', 'XML': xml}, timeout=5)
        root = ET.fromstring(r.content)
        new_addr = root.find(".//Address2").text
        new_csz = f"{root.find('.//City').text} {root.find('.//State').text} {root.find('.//Zip5').text}-{root.find('.//Zip4').text}"
        return new_addr, new_csz
    except: return addr, csz

def main():
    parser = argparse.ArgumentParser(description='Avery 15246 MTG Labeler')
    parser.add_argument('file', nargs='?')
    parser.add_argument('--start', type=int, default=0)
    parser.add_argument('--test', action='store_true')
    parser.add_argument('--validate', action='store_true')
    parser.add_argument('--clean-cache', action='store_true')
    parser.add_argument('--no-cache', action='store_true')
    parser.add_argument('--force', action='store_true')
    parser.add_argument('--pdf', nargs='?', const='ps2pdf', help='Emit PDF using ps2pdf path')
    parser.add_argument('--output', nargs='?', const='labels', help='Base filename for output')
    args = parser.parse_args()

    header_path = 'header.ps'
    if not os.path.exists(header_path):
        header_path = '/usr/local/share/mtg-labeler/header.ps'

    try:
        with open(header_path, 'r') as f:
            ps_head = f.read()
    except FileNotFoundError:
        sys.stderr.write("Error: header.ps not found\n")
        sys.exit(1)

    if args.test:
        ps_head = ps_head.replace("/DrawOutlines false def", "/DrawOutlines true def")
        ps_head = ps_head.replace("/DrawStampGuide false def", "/DrawStampGuide true def")

    ps_output = [ps_head]
    cache = load_cache()
    date_str = datetime.date.today().strftime("%Y-%m-%d")

    if args.test:
        for i in range(10):
            ps_output.append(f"{i} (SAMPLE NAME) (123 SAMPLE ST) (BOSTON MA 02110) (#TEST) ({date_str}) L")
    elif args.file:
        idx = args.start
        with open(args.file, 'r') as f:
            for line in f:
                parts = [p.strip() for p in line.split('|')]
                if len(parts) < 4: continue
                name, addr, csz, oid = parts
                key = f"{addr}|{csz}".upper()

                if args.force:
                    v_addr, v_csz = fetch_usps(addr, csz)
                elif args.clean_cache or (args.validate and key not in cache):
                    v_addr, v_csz = fetch_usps(addr, csz)
                    if not args.no_cache:
                        cache[key] = {'addr': v_addr, 'csz': v_csz}
                        save_cache(cache)
                elif key in cache:
                    v_addr, v_csz = cache[key]['addr'], cache[key]['csz']
                else:
                    v_addr, v_csz = addr, csz

                ps_output.append(f"{idx} ({escape_ps(name)}) ({escape_ps(v_addr)}) ({escape_ps(v_csz)}) ({escape_ps(oid)}) ({date_str}) L")
                idx += 1
                if idx > 9: ps_output.append("showpage"); idx = 0
    
    ps_output.append("showpage")
    final_ps = "\n".join(ps_output)

    if args.output:
        ext = ".pdf" if args.pdf else ".ps"
        out_f = args.output if args.output.endswith(ext) else args.output + ext
        if args.pdf:
            if not shutil.which(args.pdf.split()[0]):
                sys.stderr.write(f"Error: {args.pdf} not found\n")
                sys.exit(1)
            p = subprocess.Popen([args.pdf, "-", out_f], stdin=subprocess.PIPE)
            p.communicate(input=final_ps.encode('utf-8'))
        else:
            with open(out_f, 'w') as f: f.write(final_ps)
    else:
        print(final_ps)

if __name__ == "__main__": main()
