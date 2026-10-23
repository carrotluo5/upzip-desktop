package oracle;

import at.gadermaier.argon2.Argon2;
import at.gadermaier.argon2.Argon2Factory;
import at.gadermaier.argon2.Constants;
import at.gadermaier.argon2.model.Argon2Type;
import sse.org.bouncycastle.crypto.digests.Blake3Digest;
import sse.org.bouncycastle.crypto.digests.SHA3Digest;
import sse.org.bouncycastle.crypto.digests.SkeinDigest;
import sse.org.bouncycastle.crypto.engines.AESEngine;
import sse.org.bouncycastle.crypto.generators.HKDFBytesGenerator;
import sse.org.bouncycastle.crypto.modes.SICBlockCipher;
import sse.org.bouncycastle.crypto.params.Blake3Parameters;
import sse.org.bouncycastle.crypto.params.HKDFParameters;
import sse.org.bouncycastle.crypto.params.KeyParameter;
import sse.org.bouncycastle.crypto.params.ParametersWithIV;

import javax.crypto.Cipher;
import javax.crypto.spec.SecretKeySpec;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.security.MessageDigest;
import java.time.OffsetDateTime;
import java.util.*;
import java.util.zip.CRC32;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;

/**
 * Independent (non-Rust) verification oracle for Paranoia File Encryption v4 (AES-256).
 *
 * The container parsing + KDF orchestration in this file is an INDEPENDENT
 * reimplementation written from docs/FORMAT.md and the vendored official Java
 * sources (read-only) at /tmp/paranoia-pc.  The cryptographic primitives are
 * REUSED from the vendored official sources via javac -sourcepath:
 *   - sse.org.bouncycastle... HKDFBytesGenerator / SkeinDigest / SHA3Digest /
 *     AESEngine / SICBlockCipher / Blake3Digest / Blake3Parameters
 *   - at.gadermaier.argon2 (the exact Argon2id implementation shipped by the app)
 *
 * Usage:
 *   java -cp build/classes oracle.Oracle --outdir DIR --json-out FILE [--ctr bc|jce]
 *        --sample "label|encPath|password|originalPath|expectedSha256"           (expect success)
 *        --sample "label|encPath|password|originalPath||wrongpw"                 (expect wrong-password)
 */
public final class Oracle {

    static final int HEADER_SIZE = 8;
    static final int SALT_SIZE = 32;
    static final int MAC_SIZE = 32;
    static final int CHECKCODE_SIZE = 32;
    static final int MIN_FILE = HEADER_SIZE + SALT_SIZE + MAC_SIZE; // 72

    static final String VENDOR_DIR = System.getProperty("paranoia.vendor", "/tmp/paranoia-pc");

    public static void main(String[] args) throws Exception {
        Path outDir = Paths.get("/tmp/paranoia-oracle-out");
        Path jsonOut = null;
        String ctrEngine = "bc";
        boolean keepPlain = false;
        boolean parseCustomParams = false;
        List<String> samples = new ArrayList<String>();

        for (int i = 0; i < args.length; i++) {
            String a = args[i];
            if (a.equals("--outdir")) outDir = Paths.get(args[++i]);
            else if (a.equals("--json-out")) jsonOut = Paths.get(args[++i]);
            else if (a.equals("--ctr")) ctrEngine = args[++i];
            else if (a.equals("--keep-plaintext")) keepPlain = true;
            else if (a.equals("--sample")) samples.add(args[++i]);
            else if (a.equals("--parse-custom-params")) parseCustomParams = true;
            else throw new IllegalArgumentException("unknown arg: " + a);
        }
        if (parseCustomParams) {
            printCustomParamsTable();
            return;
        }
        if (samples.isEmpty()) {
            System.err.println("no --sample given; see source header for usage");
            System.exit(64);
        }
        Files.createDirectories(outDir);

        StringBuilder samplesJson = new StringBuilder();
        StringBuilder negativesJson = new StringBuilder();
        int unexpectedFailures = 0;

        for (String spec : samples) {
            String[] f = spec.split("\\|", -1);
            if (f.length < 5) throw new IllegalArgumentException("bad --sample spec: " + spec);
            String label = f[0];
            String enc = f[1];
            String pw = f[2];
            String orig = f[3];
            String expectedSha = f[4];
            String expect = f.length > 5 ? f[5] : "success";

            System.out.println("==== sample " + label + " ====");
            SampleResult r;
            try {
                r = runSample(label, Paths.get(enc), pw, orig.isEmpty() ? null : Paths.get(orig),
                        expectedSha.isEmpty() ? null : expectedSha, outDir, ctrEngine, keepPlain);
            } catch (Exception ex) {
                r = new SampleResult();
                r.label = label;
                r.encFile = enc;
                r.fatalError = ex.getClass().getSimpleName() + ": " + ex.getMessage();
                System.out.println("FATAL: " + r.fatalError);
            }

            boolean ok;
            if ("wrongpw".equals(expect)) {
                ok = r.wrongPassword;
                negativesJson.append(negativesJson.length() == 0 ? "" : ",").append(js(label)).append(":").append(r.toJson());
            } else {
                ok = r.fatalError == null && r.macMatch && r.checkCodeValid
                        && (r.expectedSha256 == null || r.sha256Match);
                samplesJson.append(samplesJson.length() == 0 ? "" : ",").append(js(label)).append(":").append(r.toJson());
            }
            System.out.println("RESULT " + label + ": " + (ok ? "OK" : "FAIL") + " (expected " + expect + ")");
            if (!ok) unexpectedFailures++;
            System.out.println();
        }

        if (jsonOut != null) {
            StringBuilder j = new StringBuilder();
            j.append("{\n");
            j.append("  \"tool\": ").append(js("paranoia-oracle-java")).append(",\n");
            j.append("  \"description\": ").append(js("Independent non-Rust implementation (Java) cross-checking Paranoia File Encryption v4 AES-256 (.enc) layout/KDF/MAC. Crypto primitives reused from vendored official Java sources; container/KDF orchestration independently reimplemented.")).append(",\n");
            j.append("  \"generatedAt\": ").append(js(OffsetDateTime.now().toString())).append(",\n");
            j.append("  \"jdk\": ").append(js(System.getProperty("java.version") + " (" + System.getProperty("java.vendor") + ")")).append(",\n");
            j.append("  \"vendoredOfficialSource\": ").append(js(VENDOR_DIR)).append(",\n");
            j.append("  \"layout\": {\n");
            j.append("    \"header\": ").append(js("8 bytes: ASCII 'SSEFE' + version(1)=4 + algorithm(1)=0 (AES-256) + customParams(1)")).append(",\n");
            j.append("    \"salt\": ").append(js("32 bytes, follows header")).append(",\n");
            j.append("    \"ciphertext\": ").append(js("AES-256-CTR (initial counter block = 16-byte nonce big-endian, +1 per 16-byte block), length = fileSize - 72")).append(",\n");
            j.append("    \"mac\": ").append(js("BLAKE3 keyed MAC (key = authKey) over nonce(16) || header(8) || salt(32) || ciphertext, 32 bytes at EOF")).append(",\n");
            j.append("    \"plaintext\": ").append(js("32-byte checkcode [a-z0-9] followed by an embedded ZIP archive; ZIP entry name = original filename")).append(",\n");
            j.append("    \"kdf\": ").append(js("l0PWHash = HKDF-Skein1024/1024(pwUtf8, salt='memorySalt', info='memoryInfo', 256); argon2 = Argon2id v0x13 t=10*2^(customParams&0xF) m=10240*2^(customParams>>4) KiB p=4 out=256; encKey = HKDF-SHA3-512(argon2, 'encKeySalt','encKeyInfo',32), authKey = HKDF-SHA3-512(argon2,'authKeySalt','authKeyInfo',32), nonce = HKDF-SHA3-512(argon2,'nonceSalt','nonceInfo',16)")).append("\n");
            j.append("  },\n");
            j.append("  \"ctrEngine\": ").append(js(ctrEngine)).append(",\n");
            j.append("  \"samples\": {").append(samplesJson).append("},\n");
            j.append("  \"negativeTests\": {").append(negativesJson).append("}\n");
            j.append("}\n");
            Path parent = jsonOut.toAbsolutePath().getParent();
            if (parent != null) Files.createDirectories(parent);
            Files.write(jsonOut, j.toString().getBytes(StandardCharsets.UTF_8));
            System.out.println("test vectors written to " + jsonOut.toAbsolutePath());
        }
        System.out.println(unexpectedFailures == 0 ? "ALL SAMPLES OK" : ("FAILURES: " + unexpectedFailures));
        System.exit(unexpectedFailures == 0 ? 0 : 3);
    }

    // ------------------------------------------------------------------ sample

    static final class SampleResult {
        String label, encFile;
        long encSize;
        String headerHex, magic;
        int version = -1, algorithmByte = -1, customParamsByte = -1;
        int tArgon = -1, mArgon = -1, pArgon = -1;
        String saltHex;
        String l0PwHashV3Hex, argon2OutHex;
        String encKeyHex, nonceHex, authKeyHex;
        String storedMacHex, computedMacHex;
        boolean macMatch;
        String checkCodeHex, checkCodeString;
        boolean checkCodeValid, wrongPassword;
        long plaintextLen, innerZipSize;
        String zipEntriesJson = "[]";
        String originalFilename;
        String decryptedFile;
        long decryptedSize;
        String decryptedSha256, expectedSha256;
        Boolean sha256Match;
        Boolean cmpWithOriginal;
        String fatalError;
        String error;

        String toJson() {
            StringBuilder b = new StringBuilder("{");
            b.append(f("label", js(label)));
            b.append(",").append(f("encFile", js(encFile)));
            b.append(",").append(f("encSize", Long.toString(encSize)));
            if (headerHex != null) b.append(",").append(f("headerHex", js(headerHex)));
            if (magic != null) b.append(",").append(f("magic", js(magic)));
            if (version >= 0) b.append(",").append(f("formatVersion", Integer.toString(version)));
            if (algorithmByte >= 0) b.append(",").append(f("algorithmByte", Integer.toString(algorithmByte)));
            if (customParamsByte >= 0) b.append(",").append(f("customParamsByte", Integer.toString(customParamsByte)));
            if (tArgon > 0) {
                b.append(",").append(f("argon2Params", "{\"type\":\"Argon2id\",\"version\":\"0x13\",\"t\":" + tArgon
                        + ",\"mKiB\":" + mArgon + ",\"p\":" + pArgon + ",\"outputLength\":256}"));
            }
            if (saltHex != null) b.append(",").append(f("saltHex", js(saltHex)));
            if (l0PwHashV3Hex != null) b.append(",").append(f("l0PWHashV3Hex", js(l0PwHashV3Hex)));
            if (argon2OutHex != null) b.append(",").append(f("argon2OutputHex", js(argon2OutHex)));
            if (encKeyHex != null) b.append(",").append(f("encKeyHex", js(encKeyHex)));
            if (nonceHex != null) b.append(",").append(f("nonceHex", js(nonceHex)));
            if (authKeyHex != null) b.append(",").append(f("authKeyHex", js(authKeyHex)));
            if (storedMacHex != null) b.append(",").append(f("storedMacHex", js(storedMacHex)));
            if (computedMacHex != null) b.append(",").append(f("computedMacHex", js(computedMacHex)));
            if (storedMacHex != null) b.append(",").append(f("macMatch", jbool(macMatch)));
            if (checkCodeHex != null) b.append(",").append(f("checkCodeHex", js(checkCodeHex)));
            if (checkCodeString != null) b.append(",").append(f("checkCodeString", js(checkCodeString)));
            if (checkCodeHex != null) b.append(",").append(f("checkCodeValid", jbool(checkCodeValid)));
            b.append(",").append(f("wrongPassword", jbool(wrongPassword)));
            if (plaintextLen > 0) b.append(",").append(f("plaintextLen", Long.toString(plaintextLen)));
            if (innerZipSize > 0) b.append(",").append(f("innerZipSize", Long.toString(innerZipSize)));
            b.append(",").append(f("zipEntries", zipEntriesJson));
            if (originalFilename != null) b.append(",").append(f("originalFilename", js(originalFilename)));
            if (decryptedFile != null) b.append(",").append(f("decryptedFile", js(decryptedFile)));
            if (decryptedSize > 0) b.append(",").append(f("decryptedSize", Long.toString(decryptedSize)));
            if (decryptedSha256 != null) b.append(",").append(f("decryptedSha256", js(decryptedSha256)));
            if (expectedSha256 != null) b.append(",").append(f("expectedSha256", js(expectedSha256)));
            if (sha256Match != null) b.append(",").append(f("sha256Match", jbool(sha256Match)));
            if (cmpWithOriginal != null) b.append(",").append(f("byteCompareWithOriginal", jbool(cmpWithOriginal)));
            if (error != null) b.append(",").append(f("error", js(error)));
            if (fatalError != null) b.append(",").append(f("fatalError", js(fatalError)));
            b.append("}");
            return b.toString();
        }
    }

    static SampleResult runSample(String label, Path enc, String password, Path original, String expectedSha,
                                  Path outDir, String ctrEngine, boolean keepPlain) throws Exception {
        SampleResult r = new SampleResult();
        r.label = label;
        r.encFile = enc.toString();
        r.encSize = Files.size(enc);
        System.out.println("decrypting " + enc + " (" + r.encSize + " bytes), password length " + password.length()
                + ", ctr engine " + ctrEngine);

        if (r.encSize < MIN_FILE) throw new IOException("file too short: " + r.encSize + " < " + MIN_FILE);

        long ciphertextLen = r.encSize - HEADER_SIZE - SALT_SIZE - MAC_SIZE;
        r.plaintextLen = ciphertextLen;
        r.innerZipSize = ciphertextLen - CHECKCODE_SIZE;

        // ---- header + salt ------------------------------------------------
        byte[] header = new byte[HEADER_SIZE];
        byte[] salt = new byte[SALT_SIZE];
        byte[] storedMac;
        byte[] checkCode = new byte[CHECKCODE_SIZE];
        byte[] computedMac = new byte[MAC_SIZE];

        Path plainTmp = outDir.resolve(label + ".plain.tmp");
        long t0 = System.nanoTime();

        try (InputStream in = new BufferedInputStream(new FileInputStream(enc.toFile()), 1 << 20)) {
            readExactly(in, header, 0, HEADER_SIZE);
            readExactly(in, salt, 0, SALT_SIZE);
            r.headerHex = hex(header);
            r.magic = new String(header, 0, 5, StandardCharsets.US_ASCII);
            r.version = header[5] & 0xFF;
            r.algorithmByte = header[6] & 0xFF;
            r.customParamsByte = header[7] & 0xFF;
            r.saltHex = hex(salt);

            if (!"SSEFE".equals(r.magic)) throw new IOException("bad magic: " + r.magic);
            if (r.version != 4) throw new IOException("unsupported format version: " + r.version);
            if (r.algorithmByte != 0) throw new IOException("unsupported algorithm byte: " + r.algorithmByte);
            r.tArgon = 10 * (1 << (r.customParamsByte & 0x0F));
            r.mArgon = 10240 * (1 << ((r.customParamsByte >> 4) & 0x0F));
            r.pArgon = 4;

            // ---- KDF ------------------------------------------------------
            long tk = System.nanoTime();
            byte[] pwBytes = normalizedPasswordBytes(password);
            byte[] l0 = hkdf(new SkeinDigest(SkeinDigest.SKEIN_1024, 1024), pwBytes,
                    "memorySalt".getBytes(StandardCharsets.UTF_8),
                    "memoryInfo".getBytes(StandardCharsets.UTF_8), 256);
            byte[] argon2Out = argon2id(l0, salt, r.tArgon, r.mArgon, r.pArgon, 256);
            byte[] encKey = hkdf(new SHA3Digest(512), argon2Out,
                    "encKeySalt".getBytes(StandardCharsets.UTF_8),
                    "encKeyInfo".getBytes(StandardCharsets.UTF_8), 32);
            byte[] authKey = hkdf(new SHA3Digest(512), argon2Out,
                    "authKeySalt".getBytes(StandardCharsets.UTF_8),
                    "authKeyInfo".getBytes(StandardCharsets.UTF_8), 32);
            byte[] nonce = hkdf(new SHA3Digest(512), argon2Out,
                    "nonceSalt".getBytes(StandardCharsets.UTF_8),
                    "nonceInfo".getBytes(StandardCharsets.UTF_8), 16);

            r.l0PwHashV3Hex = hex(l0);
            r.argon2OutHex = hex(argon2Out);
            r.encKeyHex = hex(encKey);
            r.nonceHex = hex(nonce);
            r.authKeyHex = hex(authKey);
            System.out.printf("KDF done in %.2f s%n", (System.nanoTime() - tk) / 1e9);
            System.out.println("header : " + r.headerHex + "  (magic=" + r.magic + " version=" + r.version
                    + " algorithm=" + r.algorithmByte + " customParams=0x" + String.format("%02x", r.customParamsByte) + ")");
            System.out.println("argon2 : Argon2id v0x13 t=" + r.tArgon + " m=" + r.mArgon + "KiB p=" + r.pArgon);
            System.out.println("salt   : " + r.saltHex);
            System.out.println("l0PWHashV3 : " + r.l0PwHashV3Hex);
            System.out.println("argon2 out : " + r.argon2OutHex);
            System.out.println("encKey : " + r.encKeyHex);
            System.out.println("nonce  : " + r.nonceHex);
            System.out.println("authKey: " + r.authKeyHex);

            // ---- MAC + CTR decrypt ----------------------------------------
            Blake3Digest b3 = new Blake3Digest();
            b3.init(Blake3Parameters.key(authKey));
            b3.update(nonce, 0, nonce.length);
            b3.update(header, 0, header.length);
            b3.update(salt, 0, salt.length);

            CtrEngine ctr = "jce".equalsIgnoreCase(ctrEngine)
                    ? new JceCtr(encKey, nonce) : new BcCtr(encKey, nonce);

            byte[] buf = new byte[1 << 20];
            long remaining = ciphertextLen;
            long plainPos = 0;
            try (OutputStream pout = new BufferedOutputStream(new FileOutputStream(plainTmp.toFile()), 1 << 20)) {
                while (remaining > 0) {
                    int want = (int) Math.min(buf.length, remaining);
                    readExactly(in, buf, 0, want);
                    // MAC is computed over ciphertext
                    b3.update(buf, 0, want);
                    ctr.xorInPlace(buf, want);
                    pout.write(buf, 0, want);
                    if (plainPos < CHECKCODE_SIZE) {
                        int n = (int) Math.min(want, CHECKCODE_SIZE - plainPos);
                        System.arraycopy(buf, 0, checkCode, (int) plainPos, n);
                    }
                    plainPos += want;
                    remaining -= want;
                }
            }
            storedMac = new byte[MAC_SIZE];
            readExactly(in, storedMac, 0, MAC_SIZE);
            int extra = in.read();
            if (extra != -1) System.out.println("WARNING: trailing bytes after stored MAC (first=" + extra + ")");
            b3.doFinal(computedMac, 0);

            r.storedMacHex = hex(storedMac);
            r.computedMacHex = hex(computedMac);
            r.macMatch = Arrays.equals(storedMac, computedMac);
            r.checkCodeHex = hex(checkCode);
            r.checkCodeString = new String(checkCode, StandardCharsets.ISO_8859_1);
            r.checkCodeValid = checkCodeValid(checkCode);
            r.wrongPassword = !r.checkCodeValid;
            System.out.println("stored MAC  : " + r.storedMacHex);
            System.out.println("computed MAC: " + r.computedMacHex);
            System.out.println("MAC match   : " + r.macMatch);
            System.out.println("checkcode   : " + r.checkCodeHex + " (" + printable(checkCode) + ") valid=" + r.checkCodeValid);

            if (!r.checkCodeValid) {
                r.error = "checkcode is not [a-z0-9] -> wrong password (official error: InvalidParameterException / wrongPassword)";
                System.out.println("ERROR: " + r.error);
                if (!keepPlain) Files.deleteIfExists(plainTmp);
                System.out.printf("total %.2f s%n", (System.nanoTime() - t0) / 1e9);
                return r;
            }

            // ---- ZIP extraction + CRC + SHA-256 ---------------------------
            byte[] finalEncKey = encKey, finalNonce = nonce, finalAuthKey = authKey;
            Arrays.fill(finalEncKey, (byte) 0);
            Arrays.fill(finalNonce, (byte) 0);
            Arrays.fill(finalAuthKey, (byte) 0);

            List<String> entryJsons = new ArrayList<String>();
            long firstSize = -1;
            String firstSha = null;
            Path firstOut = null;
            boolean crcAllOk = true;

            try (InputStream fin = new BufferedInputStream(new FileInputStream(plainTmp.toFile()), 1 << 20)) {
                skipExactly(fin, CHECKCODE_SIZE);
                try (ZipInputStream zis = new ZipInputStream(fin)) {
                    ZipEntry e;
                    int idx = 0;
                    while ((e = zis.getNextEntry()) != null) {
                        Path entryOut = outDir.resolve(idx == 0 ? label + ".dec" : label + "." + idx + ".dec");
                        MessageDigest sha = MessageDigest.getInstance("SHA-256");
                        CRC32 crc = new CRC32();
                        long written = 0;
                        byte[] b = new byte[1 << 20];
                        int n;
                        try (OutputStream os = new BufferedOutputStream(new FileOutputStream(entryOut.toFile()), 1 << 20)) {
                            while ((n = zis.read(b)) != -1) {
                                os.write(b, 0, n);
                                sha.update(b, 0, n);
                                crc.update(b, 0, n);
                                written += n;
                            }
                        }
                        zis.closeEntry();
                        long declaredCrc = e.getCrc();
                        boolean crcOk = declaredCrc < 0 ? true : declaredCrc == crc.getValue();
                        if (!crcOk) crcAllOk = false;
                        String entrySha = hex(sha.digest());
                        StringBuilder ej = new StringBuilder();
                        ej.append("{\"index\":").append(idx)
                          .append(",\"name\":").append(js(e.getName()))
                          .append(",\"uncompressedSize\":").append(written)
                          .append(",\"declaredSize\":").append(e.getSize())
                          .append(",\"declaredCompressedSize\":").append(e.getCompressedSize())
                          .append(",\"crc32ActualHex\":").append(js(String.format("%08x", crc.getValue())))
                          .append(",\"declaredCrcHex\":").append(js(declaredCrc < 0 ? "unknown" : String.format("%08x", declaredCrc)))
                          .append(",\"crcMatch\":").append(jbool(crcOk))
                          .append(",\"sha256\":").append(js(entrySha))
                          .append("}");
                        entryJsons.add(ej.toString());
                        System.out.println("zip entry " + idx + ": name=" + e.getName() + " size=" + written
                                + " crc=" + String.format("%08x", crc.getValue()) + " crcMatch=" + crcOk
                                + " sha256=" + entrySha);
                        if (idx == 0) {
                            firstSize = written;
                            firstSha = entrySha;
                            firstOut = entryOut;
                        }
                        idx++;
                    }
                }
            }
            if (!keepPlain) Files.deleteIfExists(plainTmp);
            r.zipEntriesJson = "[" + String.join(",", entryJsons) + "]";
            if (firstOut == null) throw new IOException("no ZIP entries found in plaintext");
            r.originalFilename = firstEntryName(r.zipEntriesJson);
            r.decryptedFile = firstOut.toString();
            r.decryptedSize = firstSize;
            r.decryptedSha256 = firstSha;

            if (expectedSha != null) {
                r.expectedSha256 = expectedSha.toLowerCase(Locale.ROOT);
                r.sha256Match = r.expectedSha256.equals(r.decryptedSha256);
                System.out.println("sha256      : " + r.decryptedSha256 + " expected " + r.expectedSha256
                        + " -> " + (r.sha256Match ? "MATCH" : "MISMATCH"));
            }
            if (original != null && Files.exists(original)) {
                r.cmpWithOriginal = filesEqual(original, firstOut);
                System.out.println("cmp with " + original + " -> " + (r.cmpWithOriginal ? "IDENTICAL" : "DIFFERENT"));
            }
            if (!r.macMatch) r.error = "BLAKE3 keyed MAC mismatch";
            System.out.printf("total %.2f s%n", (System.nanoTime() - t0) / 1e9);
        }
        return r;
    }

    static String firstEntryName(String entriesJson) {
        int k = entriesJson.indexOf("\"name\":\"");
        if (k < 0) return null;
        int start = k + 8;
        int end = start;
        StringBuilder sb = new StringBuilder();
        while (end < entriesJson.length()) {
            char c = entriesJson.charAt(end);
            if (c == '"') break;
            if (c == '\\') {
                // names in our JSON are escaped; handle the common unicode-escape and quote cases
                char n = entriesJson.charAt(end + 1);
                if (n == 'u') {
                    sb.append((char) Integer.parseInt(entriesJson.substring(end + 2, end + 6), 16));
                    end += 6;
                } else {
                    sb.append(n == 'n' ? '\n' : n == 'r' ? '\r' : n == 't' ? '\t' : n);
                    end += 2;
                }
            } else {
                sb.append(c);
                end++;
            }
        }
        return sb.toString();
    }

    // ------------------------------------------------------------------ KDF

    static byte[] normalizedPasswordBytes(String password) {
        // Reimplementation of Encryptor.convertToCodePoints + Helpers.toBytes(pw,false):
        // chars in [32,126] kept as-is; anything else replaced by its DECIMAL code point string;
        // result is UTF-8 encoded.
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < password.length(); ) {
            int cp = password.codePointAt(i);
            i += Character.charCount(cp);
            if (cp < 32 || cp > 126) sb.append(Integer.toString(cp));
            else sb.append((char) cp);
        }
        return sb.toString().getBytes(StandardCharsets.UTF_8);
    }

    static byte[] hkdf(sse.org.bouncycastle.crypto.Digest hash, byte[] ikm, byte[] salt, byte[] info, int len) {
        HKDFBytesGenerator gen = new HKDFBytesGenerator(hash);
        gen.init(new HKDFParameters(ikm, salt, info));
        byte[] out = new byte[len];
        gen.generateBytes(out, 0, len);
        return out;
    }

    static byte[] argon2id(byte[] password, byte[] salt, int t, int mKiB, int p, int outLen) {
        Argon2 a = Argon2Factory.create();
        a.setType(Argon2Type.Argon2id);
        a.setVersion(Constants.ARGON2_VERSION_13);
        a.setIterations(t);
        a.setMemoryInKiB(mKiB);
        a.setParallelism(p);
        a.setOutputLength(outLen);
        return a.hashBytes(Arrays.copyOf(password, password.length), Arrays.copyOf(salt, salt.length));
    }

    // ------------------------------------------------------------------ CTR

    interface CtrEngine {
        void xorInPlace(byte[] data, int len);
    }

    /** AES-256-CTR using the vendored official BouncyCastle SICBlockCipher(AESEngine). */
    static final class BcCtr implements CtrEngine {
        private final SICBlockCipher ctr;
        BcCtr(byte[] key, byte[] nonce) {
            ctr = new SICBlockCipher(new AESEngine());
            ctr.init(false, new ParametersWithIV(new KeyParameter(key), nonce));
        }
        public void xorInPlace(byte[] data, int len) {
            ctr.processBytes(data, 0, len, data, 0);
        }
    }

    /** AES-256-CTR using the JDK (AES/ECB/NoPadding keystream + big-endian full-width counter). */
    static final class JceCtr implements CtrEngine {
        private final Cipher aes;
        private byte[] counter;
        private final byte[] counters;
        private final byte[] keystream;
        JceCtr(byte[] key, byte[] nonce) throws Exception {
            aes = Cipher.getInstance("AES/ECB/NoPadding");
            aes.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(key, "AES"));
            counter = Arrays.copyOf(nonce, 16);
            counters = new byte[1 << 20];
            keystream = new byte[1 << 20];
        }
        public void xorInPlace(byte[] data, int len) {
            try {
                int full = len & ~15;
                if (full > 0) {
                    int blocks = full >>> 4;
                    // build counter blocks
                    for (int i = 0; i < blocks; i++) {
                        System.arraycopy(counter, 0, counters, i * 16, 16);
                        increment(counter);
                    }
                    int ksLen = aes.update(counters, 0, blocks * 16, keystream, 0);
                    if (ksLen != blocks * 16) throw new IllegalStateException("JCE AES/ECB produced " + ksLen);
                    for (int i = 0; i < full; i++) data[i] ^= keystream[i];
                }
                if (full < len) {
                    byte[] ks = aes.update(counter, 0, 16);
                    increment(counter);
                    for (int i = full; i < len; i++) data[i] ^= ks[i - full];
                }
            } catch (Exception e) {
                throw new RuntimeException(e);
            }
        }
        private static void increment(byte[] c) {
            for (int i = c.length - 1; i >= 0; i--) {
                if (++c[i] != 0) break;
            }
        }
    }

    // ------------------------------------------------------------------ IO

    static void readExactly(InputStream in, byte[] buf, int off, int len) throws IOException {
        int done = 0;
        while (done < len) {
            int n = in.read(buf, off + done, len - done);
            if (n < 0) throw new EOFException("unexpected EOF (wanted " + len + ", got " + done + ")");
            done += n;
        }
    }

    static void skipExactly(InputStream in, long n) throws IOException {
        long left = n;
        while (left > 0) {
            long s = in.skip(left);
            if (s <= 0) {
                if (in.read() < 0) throw new EOFException("unexpected EOF while skipping");
                s = 1;
            }
            left -= s;
        }
    }

    static boolean filesEqual(Path a, Path b) throws IOException {
        if (Files.size(a) != Files.size(b)) return false;
        try (InputStream ia = new BufferedInputStream(new FileInputStream(a.toFile()), 1 << 20);
             InputStream ib = new BufferedInputStream(new FileInputStream(b.toFile()), 1 << 20)) {
            byte[] ba = new byte[1 << 20];
            byte[] bb = new byte[1 << 20];
            while (true) {
                int na = readFully(ia, ba);
                int nb = readFully(ib, bb);
                if (na != nb) return false;
                if (na < 0) return true;
                for (int i = 0; i < na; i++) if (ba[i] != bb[i]) return false;
            }
        }
    }

    static int readFully(InputStream in, byte[] buf) throws IOException {
        int off = 0;
        while (off < buf.length) {
            int n = in.read(buf, off, buf.length - off);
            if (n < 0) break;
            off += n;
        }
        return off == 0 ? -1 : off;
    }

    // ------------------------------------------------------------------ misc

    static boolean checkCodeValid(byte[] cc) {
        for (byte b : cc) {
            int c = b & 0xFF;
            boolean ok = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z');
            if (!ok) return false;
        }
        return true;
    }

    static String printable(byte[] data) {
        StringBuilder sb = new StringBuilder(data.length);
        for (byte b : data) {
            int c = b & 0xFF;
            sb.append(c >= 32 && c <= 126 ? (char) c : '.');
        }
        return sb.toString();
    }

    static String hex(byte[] data) {
        char[] hexArray = "0123456789abcdef".toCharArray();
        char[] out = new char[data.length * 2];
        for (int i = 0; i < data.length; i++) {
            int v = data[i] & 0xFF;
            out[i * 2] = hexArray[v >>> 4];
            out[i * 2 + 1] = hexArray[v & 0x0F];
        }
        return new String(out);
    }

    static String js(String s) {
        StringBuilder b = new StringBuilder("\"");
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"': b.append("\\\""); break;
                case '\\': b.append("\\\\"); break;
                case '\n': b.append("\\n"); break;
                case '\r': b.append("\\r"); break;
                case '\t': b.append("\\t"); break;
                default:
                    if (c < 0x20 || c > 0x7E) b.append(String.format("\\u%04x", (int) c));
                    else b.append(c);
            }
        }
        return b.append('"').toString();
    }

    static String jbool(boolean b) { return b ? "true" : "false"; }

    static String f(String name, String rawJsonValue) { return js(name) + ":" + rawJsonValue; }

    /**
     * Print the mapping performed by the official Argon2Params.getParametersForFE
     * (Encryptor v4) for every possible customParams byte. The official code
     * performs NO range/validity check: every byte 0x00..0xff is parseable.
     * Evidence for docs/oracle-report.md and FORMAT.md error-ordering discussion.
     */
    private static void printCustomParamsTable() {
        System.out.println("customParams byte parsing (official Paranoia v4 FE, Argon2Params.getParametersForFE):");
        System.out.println("  t = 10*2^(byte & 0x0F),  m = 10240*2^((byte>>4)&0x0F) KiB,  p = 4,  version = 0x13,  out = 256 bytes");
        System.out.println("  NOTE: official code has NO range check; every value 0x00..0xFF is structurally valid.");
        System.out.printf("%-7s %-5s %-9s %-5s %-14s %s%n", "byte", "tm", "t", "mm", "m_KiB", "m_GiB");
        for (int b = 0; b <= 0xFF; b++) {
            int tm = b & 0x0F;
            int mm = (b >> 4) & 0x0F;
            long t = 10L * (1L << tm);
            long mKib = 10240L * (1L << mm);
            double mGib = mKib / 1048576.0;
            String flag = "";
            if (mm == 15) flag = "  <- ~320 GiB virtual KDF memory; allocation likely fails";
            else if (tm == 15) flag = "  <- 327680 passes; extremely slow";
            System.out.printf("0x%02x    %-5d %-9d %-5d %-14d %.3f%s%n", b, tm, t, mm, mKib, mGib, flag);
        }
    }

    private Oracle() {}
}

