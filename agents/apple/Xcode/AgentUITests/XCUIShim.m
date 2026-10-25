//
//  XCUIShim.m
//  See XCUIShim.h. Every method funnels XCTest calls through `@try`/`@catch`
//  and reports the exception reason instead of aborting the process.
//

#import "XCUIShim.h"

/// Runs `block`, returning YES on success and filling `error` with the
/// exception reason otherwise. XCTest raises `NSInternalInconsistency...`
/// (`XCTAssert`-style) exceptions for bad interactions.
static BOOL IMPRunCatching(NSString *_Nullable *_Nullable error, void (^block)(void)) {
    @try {
        block();
        return YES;
    } @catch (NSException *exception) {
        if (error != NULL) {
            NSString *reason = exception.reason ?: @"unknown XCTest failure";
            *error = [NSString stringWithFormat:@"%@ (%@)", reason, exception.name];
        }
        return NO;
    }
}

#if TARGET_OS_TV
static BOOL IMFail(NSString *_Nullable *_Nullable error, NSString *reason) {
    if (error != NULL) {
        *error = reason;
    }
    return NO;
}
#endif

@implementation IMPXCUShim

+ (nullable NSPredicate *)predicateWithFormat:(NSString *)format
                                        error:(NSString *_Nullable *_Nullable)error {
    if (format.length == 0) {
        if (error != NULL) {
            *error = @"the predicate is empty";
        }
        return nil;
    }
    __block NSPredicate *predicate = nil;
    BOOL ok = IMPRunCatching(error, ^{
        predicate = [NSPredicate predicateWithFormat:format];
    });
    return ok ? predicate : nil;
}

+ (BOOL)launchApplication:(XCUIApplication *)application error:(NSString *_Nullable *_Nullable)error {
    return IMPRunCatching(error, ^{
        [application launch];
    });
}

+ (BOOL)terminateApplication:(XCUIApplication *)application error:(NSString *_Nullable *_Nullable)error {
    return IMPRunCatching(error, ^{
        [application terminate];
    });
}

+ (BOOL)activateApplication:(XCUIApplication *)application error:(NSString *_Nullable *_Nullable)error {
    return IMPRunCatching(error, ^{
        [application activate];
    });
}

+ (BOOL)tapElement:(XCUIElement *)element error:(NSString *_Nullable *_Nullable)error {
#if TARGET_OS_TV
    // tvOS has no touch input; XCUITest marks -tap unavailable. The executor
    // uses focus + Select for normal clicks and reaches this shim only for an
    // unreachable focus trap, where an honest failure is the correct answer.
    (void)element;
    return IMFail(error, @"tap is unavailable on tvOS; use focus and Select");
#else
    return IMPRunCatching(error, ^{
        [element tap];
    });
#endif
}

+ (BOOL)pressElement:(XCUIElement *)element
         forDuration:(NSTimeInterval)duration
               error:(NSString *_Nullable *_Nullable)error {
#if TARGET_OS_TV
    // tvOS long-press uses XCUIRemote's Select long-press in Swift.
    (void)element;
    (void)duration;
    return IMFail(error, @"touch press is unavailable on tvOS; use a remote long-press");
#else
    return IMPRunCatching(error, ^{
        [element pressForDuration:duration];
    });
#endif
}

+ (BOOL)typeText:(NSString *)text
      intoElement:(XCUIElement *)element
            error:(NSString *_Nullable *_Nullable)error {
    return IMPRunCatching(error, ^{
        [element typeText:text];
    });
}

+ (BOOL)adjustPickerWheel:(XCUIElement *)element
                  toValue:(NSString *)value
                    error:(NSString *_Nullable *_Nullable)error {
#if TARGET_OS_TV
    (void)element;
    (void)value;
    return IMFail(error, @"picker wheels are unavailable on tvOS");
#else
    return IMPRunCatching(error, ^{
        [element adjustToPickerWheelValue:value];
    });
#endif
}

+ (BOOL)dragElement:(XCUIElement *)element
               from:(CGVector)from
                 to:(CGVector)to
              error:(NSString *_Nullable *_Nullable)error {
#if TARGET_OS_TV
    (void)element;
    (void)from;
    (void)to;
    return IMFail(error, @"touch drags are unavailable on tvOS; use the remote");
#else
    return IMPRunCatching(error, ^{
        XCUICoordinate *start = [element coordinateWithNormalizedOffset:from];
        XCUICoordinate *end = [element coordinateWithNormalizedOffset:to];
        [start pressForDuration:0.05 thenDragToCoordinate:end];
    });
#endif
}

@end
