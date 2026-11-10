#!/usr/bin/env python3
"""Compare Rustic against the installed native HarfBuzz (test tooling only).

Build cargo's shape_font example first. --write records the independent
reference; default verifies the committed reference and Rustic's output.
"""
from pathlib import Path
import argparse
import ctypes as C
import ctypes.util
import subprocess
ROOT=Path(__file__).resolve().parents[1]
FONTS=ROOT/'crates/rustic-text/tests/fonts/shaping'
REFERENCE=ROOT/'crates/rustic-text/tests/fixtures/shaping_reference.txt'
CASES=[('RusticCFFTest.otf',d,t) for d in ('ltr','rtl') for t in ('x','fi','AV','x\u0301','fiAV','AVfi')]+[('Arabic.ttf','rtl',t) for t in ('ب','بب','سلام','العربية','لا','ب\u200cب','ب\u200dب','ببب','مَ','مُحَمَّد')]+[('Devanagari.ttf','ltr',t) for t in ('क','कि','की','क्ष','हिन्दी','नमस्ते','प्रकाश','कर्म')]
class Info(C.Structure):_fields_=[(n,C.c_uint32) for n in ('codepoint','mask','cluster','var1','var2')]
class Position(C.Structure):_fields_=[(n,C.c_int32) for n in ('x_advance','y_advance','x_offset','y_offset','var')]
def library():
    hb=C.CDLL(ctypes.util.find_library('harfbuzz'))
    signatures={'hb_version_string':([],C.c_char_p),'hb_blob_create':([C.c_char_p,C.c_uint,C.c_int,C.c_void_p,C.c_void_p],C.c_void_p),'hb_face_create':([C.c_void_p,C.c_uint],C.c_void_p),'hb_face_get_upem':([C.c_void_p],C.c_uint),'hb_font_create':([C.c_void_p],C.c_void_p),'hb_ot_font_set_funcs':([C.c_void_p],None),'hb_font_set_scale':([C.c_void_p,C.c_int,C.c_int],None),'hb_buffer_create':([],C.c_void_p),'hb_buffer_add_utf8':([C.c_void_p,C.c_char_p,C.c_int,C.c_uint,C.c_int],None),'hb_buffer_set_direction':([C.c_void_p,C.c_int],None),'hb_buffer_set_flags':([C.c_void_p,C.c_uint],None),'hb_buffer_guess_segment_properties':([C.c_void_p],None),'hb_shape':([C.c_void_p,C.c_void_p,C.c_void_p,C.c_uint],None),'hb_buffer_get_glyph_infos':([C.c_void_p,C.POINTER(C.c_uint)],C.POINTER(Info)),'hb_buffer_get_glyph_positions':([C.c_void_p,C.POINTER(C.c_uint)],C.POINTER(Position))}
    for n in ('blob','face','font','buffer'):signatures['hb_'+n+'_destroy']=([C.c_void_p],None)
    for n,(args,result) in signatures.items():f=getattr(hb,n);f.argtypes=args;f.restype=result
    return hb
def reference(hb,path,direction,text):
    data=path.read_bytes();blob=hb.hb_blob_create(data,len(data),0,None,None);face=hb.hb_face_create(blob,0);font=hb.hb_font_create(face);hb.hb_ot_font_set_funcs(font);upem=hb.hb_face_get_upem(face);hb.hb_font_set_scale(font,upem,upem)
    buffer=hb.hb_buffer_create();b=text.encode();hb.hb_buffer_add_utf8(buffer,b,len(b),0,len(b));hb.hb_buffer_set_direction(buffer,5 if direction=='rtl' else 4);hb.hb_buffer_set_flags(buffer,8);hb.hb_buffer_guess_segment_properties(buffer);hb.hb_shape(font,buffer,None,0)
    count=C.c_uint();infos=hb.hb_buffer_get_glyph_infos(buffer,C.byref(count));positions=hb.hb_buffer_get_glyph_positions(buffer,C.byref(count))
    result=[(infos[i].codepoint,positions[i].x_advance,positions[i].x_offset,positions[i].y_offset) for i in range(count.value)]
    hb.hb_buffer_destroy(buffer);hb.hb_font_destroy(font);hb.hb_face_destroy(face);hb.hb_blob_destroy(blob);return result
def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--write',action='store_true');p.add_argument('--example',type=Path,default=ROOT/'target/debug/examples/shape_font');a=p.parse_args();hb=library();version=hb.hb_version_string().decode()
    lines=['# Native HarfBuzz '+version+'; glyph,advance,x_offset,y_offset; original Unicode17 clusters tested separately.']
    for name,direction,text in CASES:
        expected=reference(hb,FONTS/name,direction,text)
        actual=[tuple(map(float,line.split())) for line in subprocess.check_output([str(a.example),str(FONTS/name),direction,text],text=True).splitlines()]
        if actual!=expected:raise AssertionError((name,direction,text,expected,actual))
        codepoints=','.join(f'{ord(c):X}' for c in text);glyphs=';'.join(','.join(map(str,g)) for g in expected);lines.append('\t'.join((name,direction,codepoints,glyphs)))
    content='\n'.join(lines)+'\n'
    if a.write:REFERENCE.write_text(content)
    else:
        # A different installed HarfBuzz release may change its version label.
        assert REFERENCE.read_text().splitlines()[1:]==lines[1:],'reference results changed'
    print(f'{len(CASES)} shaping cases agree with native HarfBuzz {version}')
if __name__=='__main__':main()
