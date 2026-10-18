// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
//
// Runs Sparkle's own update checks (its sources, compiled by accepts.sh)
// against an installed app and a downloaded update, the way the installed
// app's Sparkle would: the archive's EdDSA signature with the old app's
// key, SUUpdateValidator's policy for the new bundle (code signing kept,
// EdDSA key kept, Apple code signature valid; the Developer ID team match
// when the old app has one), SUInstaller finding the new bundle, and
// SUPlainInstaller's no-downgrade rule on CFBundleVersion.
//
//   validate OLD.app ARCHIVE ED_SIGNATURE EXTRACTED_DIR
#import <Foundation/Foundation.h>
#import "SPUVerifierInformation.h"
#import "SUHost.h"
#import "SUInstaller.h"
#import "SUSignatures.h"
#import "SUStandardVersionComparator.h"
#import "SUUpdateValidator.h"

static int fail(NSString *what, NSError *error) {
    printf("FAIL %s: %s\n", what.UTF8String, error.description.UTF8String);
    return 1;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc != 5) {
            fprintf(stderr, "usage: %s OLD.app ARCHIVE ED_SIGNATURE EXTRACTED_DIR\n", argv[0]);
            return 2;
        }
        NSBundle *old = [NSBundle bundleWithPath:@(argv[1])];
        if (old == nil) { fprintf(stderr, "no bundle at %s\n", argv[1]); return 2; }
        SUHost *host = [[SUHost alloc] initWithBundle:old];
        NSString *archive = @(argv[2]), *dir = @(argv[4]);
        SUSignatures *signatures = [[SUSignatures alloc] initWithEd:@(argv[3])];
        NSError *error = nil;

        // SUVerifyUpdateBeforeExtraction (the Swift app sets it): the archive first.
        SPUVerifierInformation *info = [[SPUVerifierInformation alloc] initWithExpectedVersion:@"" expectedContentLength:0];
        SUUpdateValidator *before = [[SUUpdateValidator alloc] initWithDownloadPath:archive signatures:signatures host:host verifierInformation:info];
        if (![before validateHostHasPublicKeys:&error]) return fail(@"old app's public key", error);
        if (![before validateDownloadPathWithFallbackOnCodeSigning:YES error:&error]) return fail(@"archive signature", error);
        printf("PASS the archive's EdDSA signature verifies with the old app's SUPublicEDKey\n");
        if (![before validateWithUpdateDirectory:dir error:&error]) return fail(@"update (verified before extraction)", error);
        printf("PASS SUUpdateValidator accepts the new bundle (verified before extraction)\n");

        // Without it: the bundle path, EdDSA or Apple code signing matching the old app.
        SUUpdateValidator *after = [[SUUpdateValidator alloc] initWithDownloadPath:archive signatures:signatures host:host verifierInformation:info];
        if (![after validateWithUpdateDirectory:dir error:&error]) return fail(@"update (verified after extraction)", error);
        printf("PASS SUUpdateValidator accepts the new bundle (verified after extraction)\n");

        NSString *source = [SUInstaller installSourcePathInUpdateFolder:dir forHost:host isPackage:NULL isGuided:NULL];
        if (source == nil) { printf("FAIL SUInstaller finds no update for %s\n", old.bundlePath.lastPathComponent.UTF8String); return 1; }
        NSBundle *update = [NSBundle bundleWithPath:source];
        NSString *from = old.infoDictionary[@"CFBundleVersion"], *to = update.infoDictionary[@"CFBundleVersion"];
        if ([[SUStandardVersionComparator defaultComparator] compareVersion:from toVersion:to] == NSOrderedDescending) {
            printf("FAIL downgrade: CFBundleVersion %s is older than %s\n", to.UTF8String, from.UTF8String);
            return 1;
        }
        printf("PASS SUInstaller installs %s (%s) over %s, CFBundleVersion %s -> %s\n", source.lastPathComponent.UTF8String,
               update.bundleIdentifier.UTF8String, old.bundleIdentifier.UTF8String, from.UTF8String, to.UTF8String);
    }
    return 0;
}
