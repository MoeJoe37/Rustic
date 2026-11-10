#!/usr/bin/env python3
"""Generate the original M3 article's tiny PNG swatch using only stdlib."""
from pathlib import Path
import struct
import zlib
ROOT=Path(__file__).resolve().parents[1]
def chunk(tag,data):
    return struct.pack('>I',len(data))+tag+data+struct.pack('>I',zlib.crc32(tag+data))
def main():
    width,height=24,16
    colors=[(17,124,114,255),(23,48,71,255),(238,148,102,255)]
    rows=b''.join(b'\0'+b''.join(bytes(colors[min(2,x//8)]) for x in range(width)) for _ in range(height))
    header=struct.pack('>IIBBBBB',width,height,8,6,0,0,0)
    png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',header)+chunk(b'IDAT',zlib.compress(rows,9))+chunk(b'IEND',b'')
    (ROOT/'crates/rustic-cli/tests/fixtures/m3/swatch.png').write_bytes(png)
if __name__=='__main__':main()
