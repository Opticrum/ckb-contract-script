use ckb_cinnabar_verifier::{define_errors, CUSTOM_ERROR_START};

define_errors!(OpticrumError, {
    // Order errors
    BadOrderCancel = CUSTOM_ERROR_START,
    BadOrderMatch,
    ChannelCellNotInDep,
    ChannelCapacityMismatch,
    ChannelCreatedBeforeOrder,
    OrderDataNotSet,
    BadXudtAmount,

    // Match errors
    BadExtractionAmount,
    MatchDataNotSet,
    HeaderNotSet,
    BadMatchDataUpdate,
    BadMatchUpdate,
    MatchNotExhausted,
    RentPerBlockMismatch,
    MatchNotViable, // reserved — preserves error code indices

    // General errors
    BadArgsLength,
    BuyerAuthMissing,
    SellerAuthMissing,
    AuthorizationMissing,
    UnexpectedBranch,
    UnknownState,
});
