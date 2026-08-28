using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

namespace CodeHangar.Packaging
{
    public static class OfflineAuthenticode
    {
        private static readonly Guid WinTrustActionGenericVerifyV2 =
            new Guid("00AAC56B-CD44-11d0-8CC2-00C04FC295EE");

        private const uint WtdUiNone = 2;
        private const uint WtdChoiceFile = 1;
        private const uint WtdStateActionVerify = 1;
        private const uint WtdStateActionClose = 2;
        private const uint WtdRevocationCheckChainExcludeRoot = 0x00000080;
        private const uint WtdCacheOnlyUrlRetrieval = 0x00001000;

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct WinTrustFileInfo
        {
            internal uint StructSize;
            [MarshalAs(UnmanagedType.LPWStr)] internal string FilePath;
            internal IntPtr FileHandle;
            internal IntPtr KnownSubject;
        }

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct WinTrustData
        {
            internal uint StructSize;
            internal IntPtr PolicyCallbackData;
            internal IntPtr SipClientData;
            internal uint UiChoice;
            internal uint RevocationChecks;
            internal uint UnionChoice;
            internal IntPtr FileInfo;
            internal uint StateAction;
            internal IntPtr StateData;
            [MarshalAs(UnmanagedType.LPWStr)] internal string UrlReference;
            internal uint ProviderFlags;
            internal uint UiContext;
            internal IntPtr SignatureSettings;
        }

        [DllImport("wintrust.dll", ExactSpelling = true, SetLastError = true,
            CharSet = CharSet.Unicode, PreserveSig = true)]
        private static extern int WinVerifyTrust(
            IntPtr windowHandle,
            [In] ref Guid actionId,
            IntPtr trustData);

        public static void VerifyFile(string filePath)
        {
            if (string.IsNullOrWhiteSpace(filePath))
            {
                throw new ArgumentException("A file path is required.", nameof(filePath));
            }

            var fileInfo = new WinTrustFileInfo
            {
                StructSize = (uint)Marshal.SizeOf(typeof(WinTrustFileInfo)),
                FilePath = filePath,
                FileHandle = IntPtr.Zero,
                KnownSubject = IntPtr.Zero
            };
            IntPtr fileInfoPointer = IntPtr.Zero;
            IntPtr trustDataPointer = IntPtr.Zero;
            try
            {
                fileInfoPointer = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(WinTrustFileInfo)));
                Marshal.StructureToPtr(fileInfo, fileInfoPointer, false);

                var trustData = new WinTrustData
                {
                    StructSize = (uint)Marshal.SizeOf(typeof(WinTrustData)),
                    PolicyCallbackData = IntPtr.Zero,
                    SipClientData = IntPtr.Zero,
                    UiChoice = WtdUiNone,
                    RevocationChecks = 0,
                    UnionChoice = WtdChoiceFile,
                    FileInfo = fileInfoPointer,
                    StateAction = WtdStateActionVerify,
                    StateData = IntPtr.Zero,
                    UrlReference = null,
                    ProviderFlags = WtdCacheOnlyUrlRetrieval | WtdRevocationCheckChainExcludeRoot,
                    UiContext = 0,
                    SignatureSettings = IntPtr.Zero
                };
                trustDataPointer = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(WinTrustData)));
                Marshal.StructureToPtr(trustData, trustDataPointer, false);

                Guid action = WinTrustActionGenericVerifyV2;
                int result = WinVerifyTrust(new IntPtr(-1), ref action, trustDataPointer);
                var verifiedData = (WinTrustData)Marshal.PtrToStructure(
                    trustDataPointer,
                    typeof(WinTrustData));
                verifiedData.StateAction = WtdStateActionClose;
                Marshal.StructureToPtr(verifiedData, trustDataPointer, true);
                WinVerifyTrust(new IntPtr(-1), ref action, trustDataPointer);

                if (result != 0)
                {
                    throw new Win32Exception(result,
                        "Offline WinVerifyTrust rejected the embedded Authenticode signature.");
                }
            }
            finally
            {
                if (trustDataPointer != IntPtr.Zero)
                {
                    Marshal.DestroyStructure(trustDataPointer, typeof(WinTrustData));
                    Marshal.FreeHGlobal(trustDataPointer);
                }
                if (fileInfoPointer != IntPtr.Zero)
                {
                    Marshal.DestroyStructure(fileInfoPointer, typeof(WinTrustFileInfo));
                    Marshal.FreeHGlobal(fileInfoPointer);
                }
            }
        }
    }
}
