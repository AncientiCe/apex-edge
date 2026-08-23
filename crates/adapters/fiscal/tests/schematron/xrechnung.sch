<?xml version="1.0" encoding="UTF-8"?>
<schema xmlns="http://purl.oclc.org/dsdl/schematron">
  <pattern id="en16931-ubl">
    <rule context="Invoice">
      <assert test="cbc:CustomizationID">BT-24 CustomizationID is required</assert>
      <assert test="cbc:ID">BT-1 Invoice number is required</assert>
      <assert test="cbc:IssueDate">BT-2 Issue date is required</assert>
      <assert test="cbc:InvoiceTypeCode">BT-3 Invoice type code is required</assert>
      <assert test="cbc:DocumentCurrencyCode">BT-5 Currency is required</assert>
      <assert test="cac:AccountingSupplierParty">BT-27 Seller is required</assert>
      <assert test="cac:AccountingCustomerParty">BT-44 Buyer is required</assert>
      <assert test="cac:TaxTotal">BT-110 Tax total is required</assert>
      <assert test="cac:LegalMonetaryTotal">BT-109 Document totals are required</assert>
      <assert test="cac:InvoiceLine">At least one invoice line is required</assert>
      <assert test="contains(cbc:CustomizationID,'xrechnung')">XRechnung profile id must be present</assert>
    </rule>
  </pattern>
</schema>
