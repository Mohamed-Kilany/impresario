//
//  XCUIShim.h
//  The Objective-C shim of the XCUITest bundle (spec 12.3, I-85).
//
//  Two jobs, both impossible in pure Swift:
//
//   1. build `NSPredicate`s from play-provided formats without letting an
//      `NSException` (invalid syntax, bad substitution) kill the test process —
//      Swift cannot catch Objective-C exceptions;
//   2. wrap every XCTest interaction that can raise, so a bad element (gone,
//      not hittable, not editable) becomes a normal `ok=false` result instead
//      of a crashed test process.
//
//  The names are pinned with `NS_SWIFT_NAME` so the Swift call sites are
//  stable across SDK renames.
//

#import <Foundation/Foundation.h>
#import <XCTest/XCTest.h>
// Xcode 26 splits the UI-automation classes into their own framework; XCTest
// re-exports them on older SDKs. Import both so the shim builds on either.
#if __has_include(<XCUIAutomation/XCUIAutomation.h>)
#import <XCUIAutomation/XCUIAutomation.h>
#endif

NS_ASSUME_NONNULL_BEGIN

@interface IMPXCUShim : NSObject

/// `nil` when `format` is not a valid predicate; `error` carries the reason.
+ (nullable NSPredicate *)predicateWithFormat:(NSString *)format
                                        error:(NSString *_Nullable *_Nullable)error
    NS_SWIFT_NAME(predicate(format:error:));

+ (BOOL)launchApplication:(XCUIApplication *)application
                    error:(NSString *_Nullable *_Nullable)error
    NS_SWIFT_NAME(launch(_:error:));

+ (BOOL)terminateApplication:(XCUIApplication *)application
                       error:(NSString *_Nullable *_Nullable)error
    NS_SWIFT_NAME(terminate(_:error:));

+ (BOOL)activateApplication:(XCUIApplication *)application
                      error:(NSString *_Nullable *_Nullable)error
    NS_SWIFT_NAME(activate(_:error:));

+ (BOOL)tapElement:(XCUIElement *)element
             error:(NSString *_Nullable *_Nullable)error
    NS_SWIFT_NAME(tap(_:error:));

+ (BOOL)pressElement:(XCUIElement *)element
         forDuration:(NSTimeInterval)duration
               error:(NSString *_Nullable *_Nullable)error
    NS_SWIFT_NAME(press(_:forDuration:error:));

+ (BOOL)typeText:(NSString *)text
      intoElement:(XCUIElement *)element
            error:(NSString *_Nullable *_Nullable)error
    NS_SWIFT_NAME(type(_:into:error:));

+ (BOOL)adjustPickerWheel:(XCUIElement *)element
                  toValue:(NSString *)value
                    error:(NSString *_Nullable *_Nullable)error
    NS_SWIFT_NAME(adjustPickerWheel(_:to:error:));

/// One 80%-of-the-extent drag inside `element` (spec 5.6), offsets in the
/// element's normalized coordinate space.
+ (BOOL)dragElement:(XCUIElement *)element
               from:(CGVector)from
                 to:(CGVector)to
              error:(NSString *_Nullable *_Nullable)error
    NS_SWIFT_NAME(drag(_:from:to:error:));

@end

NS_ASSUME_NONNULL_END
