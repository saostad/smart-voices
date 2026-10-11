import Testing
@testable import SmartVoicesCore

@Test func versionIsSet() {
    #expect(!SmartVoicesCore.version.isEmpty)
}
