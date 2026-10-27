# Regenerates the encrypted test fixtures: python make_fixtures.py <this dir>
# Requires: pip install pypdf cryptography
import io, sys
from pypdf import PdfReader, PdfWriter
from pypdf.constants import UserAccessPermissions as P

def base_pdf():
    objs = []
    font = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"
    pages = []
    for i in (1, 2):
        content = f"BT /F1 24 Tf 20 50 Td (Hello FlattenPDF {i}) Tj ET"
        pages.append(content)
    # 1 catalog, 2 pages, 3 font, 4/5 page1 + content, 6/7 page2 + content
    objs.append("<< /Type /Catalog /Pages 2 0 R >>")
    objs.append("<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>")
    objs.append(font)
    for n, c in zip((4, 6), pages):
        objs.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 100] /Resources << /Font << /F1 3 0 R >> >> /Contents {n+1} 0 R >>")
        objs.append(f"<< /Length {len(c)} >>\nstream\n{c}\nendstream")
    out = bytearray(b"%PDF-1.7\n")
    offs = []
    for i, o in enumerate(objs, 1):
        offs.append(len(out)); out += f"{i} 0 obj\n{o}\nendobj\n".encode()
    x = len(out)
    out += f"xref\n0 {len(objs)+1}\n0000000000 65535 f \n".encode()
    for o in offs: out += f"{o:010} 00000 n \n".encode()
    out += f"trailer\n<< /Size {len(objs)+1} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()
    return bytes(out)

def encrypt(name, user, owner, algo, perms):
    w = PdfWriter(clone_from=PdfReader(io.BytesIO(base_pdf())))
    w.add_outline_item("Second page", 1)
    w.encrypt(user_password=user, owner_password=owner, algorithm=algo, permissions_flag=perms)
    with open(f"{sys.argv[1]}/{name}", "wb") as f: w.write(f)

encrypt("user-password-rc4.pdf", "secret", "owner", "RC4-128", P.PRINT)
# Opens without a password, but printing/copying/editing are restricted.
encrypt("owner-restricted-aes256.pdf", "", "owner", "AES-256", P(0))
print("ok")
