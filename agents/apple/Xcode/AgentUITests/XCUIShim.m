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
    return IMPRunCatching(error, ^{
        [element tap];
    });
}

+ (BOOL)pressElement:(XCUIElement *)element
         forDuration:(NSTimeInterval)duration
               error:(NSString *_Nullable *_Nullable)error {
    return IMPRunCatching(error, ^{
        [element pressForDuration:duration];
    });
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
    return IMPRunCatching(error, ^{
        [element adjustToPickerWheelValue:value];
    });
}

+ (BOOL)dragElement:(XCUIElement *)element
               from:(CGVector)from
                 to:(CGVector)to
              error:(NSString *_Nullable *_Nullable)error {
    return IMPRunCatching(error, ^{
        XCUICoordinate *start = [element coordinateWithNormalizedOffset:from];
        XCUICoordinate *end = [element coordinateWithNormalizedOffset:to];
        [start pressForDuration:0.05 thenDragToCoordinate:end];
    });
}

@end
