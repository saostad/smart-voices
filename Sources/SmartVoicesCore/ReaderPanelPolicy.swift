/// A panel closed during a reading stays closed until the next reading.
public enum ReaderPanelPolicy {
    public static func shouldShow(
        isActive: Bool, showSetting: Bool, dismissedReadingID: Int?, currentReadingID: Int
    ) -> Bool {
        isActive && showSetting && dismissedReadingID != currentReadingID
    }
}
